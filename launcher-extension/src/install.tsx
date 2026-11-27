import { Action, ActionPanel, closeMainWindow, List, PopToRootType } from "@vicinae/api";
import { spawn } from "node:child_process";
import { useRef, useState } from "react";
import { asset, bridgePath, useSearch } from "./bridge";

type App = { id: number; name: string; icon: string | null };
type Status = "Installing" | "Installed ✓" | "Failed";
const noIcon = asset("no-icon.webp");

export default function InstallApps() {
  const { apps, loading, failed, setQuery } = useSearch<App>("catalog");
  const [statuses, setStatuses] = useState<Record<number, Status>>({});
  const installing = useRef(new Set<number>());

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
          accessories={[statuses[app.id] ? { text: statuses[app.id] } : { icon: asset("install.svg") }]}
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
