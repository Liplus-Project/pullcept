// The thread config carries only a disposable path, never room credentials.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const [file, runner, entry] = process.argv.slice(2);
try {
  const values = JSON.parse(readFileSync(file, "utf8"));
  for (const [key, value] of Object.entries(values)) {
    if (!key.startsWith("PULLCEPT_") || typeof value !== "string") throw new Error();
    process.env[key] = value;
  }
  writeFileSync(file.replace(/\.json$/, ".ready"), "ready", {mode: 0o600});
  process.argv = [process.execPath, runner, entry];
  await import(pathToFileURL(runner).href);
} catch {
  // Do not expose native arguments, file contents, URLs, or credentials.
  process.stderr.write("Pullcept seat admission is no longer available.\n");
  process.exitCode = 1;
}
