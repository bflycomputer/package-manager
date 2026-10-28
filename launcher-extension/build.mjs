import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { build } from "esbuild";

process.chdir(import.meta.dirname);
const output = execFileSync("cargo", [
  "build", "--release", "--locked", "-p", "pm-extension-bridge", "--message-format=json",
], { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
const artifact = output.trim().split("\n").map((line) => JSON.parse(line))
  .find((message) => message.target?.name === "pm-extension-bridge" && message.executable);
if (!artifact) throw new Error("Cargo did not produce pm-extension-bridge");

rmSync("dist", { recursive: true, force: true });
await build({
  entryPoints: ["src/install.tsx"],
  bundle: true,
  platform: "node",
  format: "cjs",
  jsx: "automatic",
  external: ["@vicinae/api", "react", "react/jsx-runtime"],
  outdir: "dist",
});
mkdirSync("dist/bin", { recursive: true });
cpSync("assets", "dist/assets", { recursive: true });
cpSync(artifact.executable, "dist/bin/pm-extension-bridge");
cpSync("package.json", "dist/package.json");
cpSync("../LICENSE", "dist/LICENSE");
cpSync("../libpm/LICENSES", "dist/LICENSES", { recursive: true });
