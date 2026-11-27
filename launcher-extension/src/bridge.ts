import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { useEffect, useRef, useState } from "react";

export const bridgePath = join(__dirname, "bin", "pm-extension-bridge");
export const asset = (name: string) => join(__dirname, "assets", name);

export function useSearch<App>(mode: "catalog" | "installed") {
  const [query, setQuery] = useState("");
  const [apps, setApps] = useState<App[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const search = useRef<ChildProcessWithoutNullStreams | null>(null);
  const latestQuery = useRef(query);
  latestQuery.current = query;

  useEffect(() => {
    let active = true;
    const child = spawn(bridgePath, [mode]);
    search.current = child;
    const fail = () => {
      if (!active) return;
      setApps([]);
      setFailed(true);
      setLoading(false);
    };
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      try {
        const reply = JSON.parse(line) as { query: string; apps?: App[]; error?: boolean };
        if (!active || reply.query !== latestQuery.current) return;
        setApps(reply.apps ?? []);
        setFailed(Boolean(reply.error));
        setLoading(false);
      } catch {
        fail();
      }
    });
    child.stderr.on("data", (data) => console.error(String(data)));
    child.on("error", fail);
    child.on("exit", fail);
    child.stdin.on("error", fail);
    return () => {
      active = false;
      search.current = null;
      lines.close();
      child.kill();
    };
  }, [mode]);

  useEffect(() => {
    const child = search.current;
    setApps([]);
    if (!child || child.exitCode !== null || child.stdin.destroyed) {
      setFailed(true);
      setLoading(false);
      return;
    }
    setFailed(false);
    setLoading(true);
    child.stdin.write(`${JSON.stringify(query)}\n`);
  }, [query]);

  return { apps, loading, failed, setQuery };
}
