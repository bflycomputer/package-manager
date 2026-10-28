import { Action, ActionPanel, closeMainWindow, List, PopToRootType } from "@vicinae/api";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { useEffect, useRef, useState } from "react";

type App = { id: number; name: string; icon: string | null };
type Status = "Installing" | "Installed ✓" | "Failed";
const bridgePath = join(__dirname, "bin", "pm-extension-bridge");
const noIcon = join(__dirname, "assets", "no-icon.webp");

export default function InstallApps() {
  const [query, setQuery] = useState("");
  const [apps, setApps] = useState<App[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [statuses, setStatuses] = useState<Record<number, Status>>({});
  const search = useRef<ChildProcessWithoutNullStreams | null>(null);
  const latestQuery = useRef(query);
  const installing = useRef(new Set<number>());
  latestQuery.current = query;

  useEffect(() => {
    let active = true;
    const child = spawn(bridgePath, ["search"]);
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
  }, []);

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

  async function install(app: App) {
    if (installing.current.has(app.id) || statuses[app.id] === "Installed ✓") return;
    installing.current.add(app.id);
    setStatuses((current) => ({ ...current, [app.id]: "Installing" }));
    const finish = (status: Status) => {
      installing.current.delete(app.id);
      setStatuses((current) => ({ ...current, [app.id]: status }));
    };
    try {
      // Release launcher focus before a package manager opens authentication.
      await closeMainWindow({ popToRootType: PopToRootType.Suspended });
    } catch (error) {
      console.error(error);
      finish("Failed");
      return;
    }
    // Install output is separate from the search protocol, and leaving the view
    // must not terminate the package manager midway through a transaction.
    const child = spawn(bridgePath, ["install", String(app.id)], {
      detached: true,
      stdio: "ignore",
    });
    child.once("error", () => finish("Failed"));
    child.once("exit", (code) => finish(code === 0 ? "Installed ✓" : "Failed"));
    child.unref();
  }

  return (
    <List filtering={false} isLoading={loading} searchBarPlaceholder="Install:" onSearchTextChange={setQuery}>
      <List.EmptyView title={failed ? "Failed" : loading ? "Loading…" : "No apps found"} />
      {apps.map((app) => (
        <List.Item
          key={app.id}
          id={String(app.id)}
          title={app.name}
          icon={app.icon ? { source: app.icon, fallback: noIcon } : noIcon}
          accessories={[{ text: statuses[app.id] ?? "Install" }]}
          actions={
            <ActionPanel>
              {!installing.current.has(app.id) && statuses[app.id] !== "Installed ✓" && (
                <Action title="Install" onAction={() => install(app)} />
              )}
            </ActionPanel>
          }
        />
      ))}
    </List>
  );
}
