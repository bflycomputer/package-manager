#!/usr/bin/env python3
"""Run milli and compare it with the frozen results from Test 1 and Test 2."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import itertools
import json
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator


TESTS = Path(__file__).resolve().parent
ROOT = TESTS.parents[1]
RUNTIME = ROOT / "tmp/engine/.runtime"
SHARD_COUNT = 12

TEST_1 = TESTS / "test-1-results.jsonl.gz"
TEST_1_SHA256 = "70664bd338374470fe26e1e0fb76806bc4304ff66cffce11e13110bcf0ccd4ec"
TEST_2 = TESTS / "test-2-results.jsonl.gz"
TEST_2_SHA256 = "8d9d895c541eedbb43245addb45aa0cec385637ac9c5ae94e51536caad3001b8"


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_fixture(path: Path, expected_sha256: str) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    if sha256_file(path) != expected_sha256:
        raise RuntimeError(f"{path.name} has changed")
    with gzip.open(path, "rt", encoding="utf-8") as source:
        metadata = json.loads(next(source))["_meta"]
        catalog = json.loads(next(source))["_catalog"]
    if len(catalog) == 0:
        raise RuntimeError(f"{path.name} contains an empty catalog")
    return metadata, catalog


def result_records(path: Path) -> Iterator[dict[str, Any]]:
    with gzip.open(path, "rt", encoding="utf-8") as source:
        next(source)
        next(source)
        for line_number, line in enumerate(source, 1):
            try:
                yield json.loads(line)
            except json.JSONDecodeError as error:
                raise RuntimeError(
                    f"invalid result {line_number} in {path.name}: {error}"
                ) from error


def write_catalog(
    directory: Path,
    metadata: dict[str, Any],
    catalog: list[dict[str, Any]],
    *,
    indent: int | None,
) -> Path:
    catalog_path = directory / "catalog.json"
    catalog_bytes = (
        json.dumps(
            catalog,
            ensure_ascii=False,
            indent=indent,
            separators=None if indent else (",", ":"),
        )
        + "\n"
    ).encode()
    expected_hash = metadata["fixture"]["catalog_sha256"]
    actual_hash = sha256_bytes(catalog_bytes)
    if actual_hash != expected_hash:
        raise RuntimeError(
            f"embedded catalog hash {actual_hash} does not match {expected_hash}"
        )
    catalog_path.write_bytes(catalog_bytes)
    (directory / "catalog.manifest.json").write_text(
        json.dumps({"catalog": {"sha256": actual_hash}}) + "\n",
        encoding="utf-8",
    )
    return catalog_path


def build_index(binary: Path, directory: Path, catalog: Path) -> Path:
    index = directory / "stripped-index"
    subprocess.run([binary, "build", index, catalog], check=True)
    return index


def first_difference(expected: list[str], actual: list[str]) -> int | None:
    for rank, pair in enumerate(
        itertools.zip_longest(expected, actual, fillvalue=None), 1
    ):
        if pair[0] != pair[1]:
            return rank
    return None


def test_1_mismatch(
    number: int, expected: dict[str, Any], actual: dict[str, Any]
) -> str:
    rank = first_difference(expected.get("top_ids", []), actual.get("top_ids", []))
    location = "after the stored top 100" if rank is None else f"at rank {rank}"
    return (
        f"Test 1 mismatch for query {expected.get('query')!r} at result {number}, {location}\n"
        f"reference total/hash: {expected.get('total_hits')}/"
        f"{expected.get('ordered_ids_sha256')}\n"
        f"milli total/hash:     {actual.get('total_hits')}/"
        f"{actual.get('ordered_ids_sha256')}"
    )


def run_test_1(binary: Path, workspace: Path) -> None:
    metadata, catalog = load_fixture(TEST_1, TEST_1_SHA256)
    directory = workspace / "test-1"
    directory.mkdir()
    index = build_index(
        binary, directory, write_catalog(directory, metadata, catalog, indent=2)
    )
    queries = directory / "queries.jsonl"
    with queries.open("w", encoding="utf-8") as destination:
        for record in result_records(TEST_1):
            destination.write(
                json.dumps({"query": record["query"]}, ensure_ascii=False) + "\n"
            )

    started = time.perf_counter()
    process = subprocess.Popen(
        [binary, "batch", index, queries],
        stdout=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    assert process.stdout is not None
    actual_lines = iter(process.stdout)
    count = 0
    try:
        for count, expected in enumerate(result_records(TEST_1), 1):
            try:
                actual = json.loads(next(actual_lines))
            except StopIteration as error:
                raise RuntimeError(f"milli stopped after {count - 1} Test 1 results") from error
            if actual != expected:
                raise RuntimeError(test_1_mismatch(count, expected, actual))
        try:
            extra = next(actual_lines)
        except StopIteration:
            pass
        else:
            raise RuntimeError(f"milli emitted an extra Test 1 result: {extra[:200]!r}")
    except BaseException:
        process.terminate()
        process.wait()
        raise
    finally:
        process.stdout.close()
    if process.wait() != 0:
        raise RuntimeError("milli's Test 1 process failed")

    expected_count = metadata["fixture"]["query_count"]
    if count != expected_count:
        raise RuntimeError(f"compared {count} Test 1 results; expected {expected_count}")
    elapsed = time.perf_counter() - started
    print(
        f"PASS Test 1: {count:,} complete ordered results in {elapsed:.3f}s "
        f"({count / elapsed:.1f} queries/s)",
        flush=True,
    )


def make_test_2_shards(directory: Path) -> tuple[list[Path], list[Path], int]:
    query_paths = [directory / f"queries-{shard:02d}.jsonl" for shard in range(SHARD_COUNT)]
    expected_paths = [directory / f"expected-{shard:02d}.jsonl" for shard in range(SHARD_COUNT)]
    query_files = [path.open("w", encoding="utf-8") for path in query_paths]
    expected_files = [path.open("w", encoding="utf-8") for path in expected_paths]
    count = 0
    try:
        for count, record in enumerate(result_records(TEST_2), 1):
            shard = (count - 1) % SHARD_COUNT
            query_files[shard].write(
                json.dumps({"query": record["query"]}, ensure_ascii=False) + "\n"
            )
            expected_files[shard].write(
                json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n"
            )
    finally:
        for file in query_files + expected_files:
            file.close()
    return query_paths, expected_paths, count


def run_test_2_processes(
    binary: Path, index: Path, queries: list[Path], directory: Path
) -> tuple[list[Path], float]:
    outputs = [directory / f"actual-{shard:02d}.jsonl" for shard in range(SHARD_COUNT)]
    errors = [directory / f"error-{shard:02d}.txt" for shard in range(SHARD_COUNT)]
    processes: list[subprocess.Popen[bytes]] = []
    for query_path, output_path, error_path in zip(queries, outputs, errors, strict=True):
        output = output_path.open("wb")
        error = error_path.open("wb")
        process = subprocess.Popen(
            [binary, index, query_path, "--exhaustive"],
            stdout=output,
            stderr=error,
        )
        output.close()
        error.close()
        processes.append(process)

    started = time.perf_counter()
    next_update = started + 30
    while any(process.poll() is None for process in processes):
        now = time.perf_counter()
        if now >= next_update:
            finished = sum(process.poll() is not None for process in processes)
            print(
                f"Test 2: {now - started:.0f}s elapsed; "
                f"{finished}/{SHARD_COUNT} shards finished",
                flush=True,
            )
            next_update = now + 30
        time.sleep(0.25)

    failures = []
    for shard, (process, error_path) in enumerate(zip(processes, errors, strict=True)):
        if process.returncode:
            failures.append(
                f"shard {shard} exited {process.returncode}:\n"
                f"{error_path.read_text(encoding='utf-8', errors='replace')}"
            )
    if failures:
        raise RuntimeError("milli's Test 2 process failed:\n" + "\n".join(failures))
    return outputs, time.perf_counter() - started


def actual_test_2_records(path: Path) -> Iterator[dict[str, Any]]:
    with path.open("r", encoding="utf-8") as source:
        for line in source:
            event = json.loads(line)
            if event["kind"] == "sample":
                yield {
                    "query": event["query"],
                    "total_hits": event["total_hits"],
                    "checksum": event["checksum"],
                }


def compare_test_2(expected_paths: list[Path], actual_paths: list[Path]) -> int:
    count = 0
    for shard, (expected_path, actual_path) in enumerate(
        zip(expected_paths, actual_paths, strict=True)
    ):
        with expected_path.open("r", encoding="utf-8") as expected_file:
            expected_records = (json.loads(line) for line in expected_file)
            for local_number, pair in enumerate(
                itertools.zip_longest(expected_records, actual_test_2_records(actual_path)), 1
            ):
                expected, actual = pair
                if expected != actual:
                    raise RuntimeError(
                        f"Test 2 mismatch in shard {shard} at result {local_number}:\n"
                        f"reference: {expected!r}\n"
                        f"milli:     {actual!r}"
                    )
                count += 1
    return count


def run_test_2(index_binary: Path, benchmark_binary: Path, workspace: Path) -> None:
    metadata, catalog = load_fixture(TEST_2, TEST_2_SHA256)
    directory = workspace / "test-2"
    directory.mkdir()
    index = build_index(
        index_binary,
        directory,
        write_catalog(directory, metadata, catalog, indent=None),
    )
    queries, expected, written = make_test_2_shards(directory)
    expected_count = metadata["fixture"]["query_count"]
    if written != expected_count:
        raise RuntimeError(f"loaded {written} Test 2 results; expected {expected_count}")

    actual, elapsed = run_test_2_processes(
        benchmark_binary, index, queries, directory
    )
    compared = compare_test_2(expected, actual)
    if compared != expected_count:
        raise RuntimeError(f"compared {compared} Test 2 results; expected {expected_count}")
    print(
        f"PASS Test 2: {compared:,} complete ordered results in {elapsed:.3f}s "
        f"({compared / elapsed:.1f} queries/s)",
        flush=True,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--index-binary",
        type=Path,
        default=RUNTIME / "target-stripped/release/libpm-search-verifier",
        help="test-only binary that builds an index and runs Test 1 batches",
    )
    parser.add_argument(
        "--benchmark-binary",
        type=Path,
        default=RUNTIME / "target-stripped/release/benchmark-stripped",
        help="test-only exhaustive milli benchmark binary",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    for binary in (args.index_binary, args.benchmark_binary):
        if not binary.is_file():
            raise SystemExit(f"required test binary is missing: {binary}")

    started = time.perf_counter()
    with tempfile.TemporaryDirectory(prefix="libpm-search-tests-") as temporary:
        workspace = Path(temporary)
        run_test_1(args.index_binary, workspace)
        run_test_2(args.index_binary, args.benchmark_binary, workspace)
    print(f"PASS: both tests completed in {time.perf_counter() - started:.3f}s")


if __name__ == "__main__":
    main()
