import { Action, ActionPanel, Alert, closeMainWindow, confirmAlert, List, PopToRootType } from "@vicinae/api";
import { spawn } from "node:child_process";
import { useState } from "react";
import { asset, bridgePath, useSearch } from "./bridge";

type App = { id: number; name: string; icon: string | null; source: { manager: string; package: string } };

const noIcon = asset("no-icon.webp");
const icon = (app: App) => (app.icon ? { source: app.icon, fallback: noIcon } : noIcon);

export default function UninstallApps() {
  const { apps, loading, failed, setQuery } = useSearch<App>("installed");
  const [hidden, setHidden] = useState<number[]>([]);

  async function uninstall(app: App) {
    const confirmed = await confirmAlert({
      title: `Uninstall ${app.name}?`,
      message: "This will remove the app from your computer.",
      icon: icon(app),
      primaryAction: { title: "Uninstall", style: Alert.ActionStyle.Destructive },
    });
    if (!confirmed) return;
    try {
      await closeMainWindow({ popToRootType: PopToRootType.Suspended });
    } catch (error) {
      console.error(error);
      return;
    }
    const restore = () => setHidden((current) => current.filter((id) => id !== app.id));
    setHidden((current) => [...current, app.id]);
    const child = spawn(bridgePath, ["uninstall", app.source.manager, app.source.package], {
      detached: true,
      stdio: "ignore",
    });
    child.once("error", restore);
    child.once("exit", (code) => code !== 0 && restore());
    child.unref();
  }

  return (
    <List filtering={false} isLoading={loading} searchBarPlaceholder="Uninstall:" onSearchTextChange={setQuery}>
      <List.EmptyView title={failed ? "Failed" : loading ? "Loading…" : "No apps found"} />
      {apps
        .filter((app) => !hidden.includes(app.id))
        .map((app) => (
          <List.Item
            key={app.id}
            id={String(app.id)}
            title={app.name}
            icon={icon(app)}
            accessories={[{ icon: asset("trash.svg") }]}
            actions={
              <ActionPanel>
                <Action title="Uninstall" onAction={() => uninstall(app)} />
              </ActionPanel>
            }
          />
        ))}
    </List>
  );
}
