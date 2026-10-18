// The app's own webhook receiver (#169).
//
// Runs the published `github-webhook-mcp` bridge unchanged, with the app on the
// other end of its stdio instead of a CLI. The bridge does not ask who its
// client is: it declares `claude/channel` and pushes
// `notifications/claude/channel` to whoever connected, so the app reads the
// same push a session would.
//
// This file adds one thing, and it is the reason the app does not spawn the
// bridge directly: the bridge does not end when its stdin does. Its stdio
// transport listens for `data` and `error` and never for `end`, and once a
// token file is there its WebSocket and keepalive timer hold the process up —
// so a bridge whose app has gone (closed, reloaded, or crashed) would stay
// running and keep receiving. Without a token file it has nothing open and
// exits on its own, which is why that case proves nothing either way. Ending on
// stdin EOF ties its life to the app's pipe, which the OS closes however the
// app went.
//
// Plain `node` runs this, not `tsx`: there is nothing here to compile, and the
// bridge is resolved from this file's own place in the tree, so it is Pullcept's
// `node_modules` that is searched and never the user's.

process.stdin.on("end", () => process.exit(0));

await import("github-webhook-mcp");
