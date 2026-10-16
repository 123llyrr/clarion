You can communicate with the running clarion instance over an IPC socket.
Check `clarion msg --help` for available commands.

The `--json` flag prints the response in JSON, rather than formatted.
For example, `clarion msg --json outputs`.

> [!TIP]
> If you're getting parsing errors from `clarion msg` after upgrading clarion, make sure that you've restarted clarion itself.
> You might be trying to run a newer `clarion msg` against an older `clarion` compositor.

### Event Stream

<sup>Since: 0.1.9</sup>

While most clarion IPC requests return a single response, the event stream request will make clarion continuously stream events into the IPC connection until it is closed.
This is useful for implementing various bars and indicators that update as soon as something happens, without continuous polling.

The event stream IPC is designed to give you the complete current state up-front, then follow up with updates to that state.
This way, your state can never "desync" from clarion, and you don't need to make any other IPC information requests.

Where reasonable, event stream state updates are atomic, though this is not always the case.
For example, a window may end up with a workspace id for a workspace that had already been removed.
This can happen if the corresponding workspaces-changed event arrives before the corresponding window-changed event.

To get a taste of the events, run `clarion msg event-stream`.
Though, this is more of a debug function than anything.
You can get raw events from `clarion msg --json event-stream`, or by connecting to the clarion socket and requesting an event stream manually.

You can find the full list of events along with documentation [here](https://github.com/123llyrr/realand/blob/main/clarion-ipc/src/lib.rs).

### Programmatic Access

`clarion msg --json` is a thin wrapper over writing and reading to a socket.
When implementing more complex scripts and modules, you're encouraged to access the socket directly.

Connect to the UNIX domain socket located at `$CLARION_SOCKET` in the filesystem.
Write your request encoded in JSON on a single line, followed by a newline character, or by flushing and shutting down the write end of the connection.
Read the reply as JSON, also on a single line.

You can use `socat` to test communicating with clarion directly:

```sh
$ socat STDIO "$CLARION_SOCKET"
"FocusedWindow"
{"Ok":{"FocusedWindow":{"id":12,"title":"t socat STDIO /run/u ~","app_id":"Alacritty","workspace_id":6,"is_focused":true}}}
```

The reply is an `Ok` or an `Err` wrapping the same JSON object as you get from `clarion msg --json`.

For more complex requests, you can use `socat` to find how `clarion msg` formats them:

```sh
$ socat STDIO UNIX-LISTEN:temp.sock
# then, in a different terminal:
$ env CLARION_SOCKET=./temp.sock clarion msg action focus-workspace 2
# then, look in the socat terminal:
{"Action":{"FocusWorkspace":{"reference":{"Index":2}}}}
```

You can find all available requests and response types in the [clarion-ipc sub-crate documentation](https://github.com/123llyrr/realand/tree/main/clarion-ipc).

### Backwards Compatibility

The JSON output *should* remain stable, as in:

- existing fields and enum variants should not be renamed
- non-optional existing fields should not be removed

However, new fields and enum variants will be added, so you should handle unknown fields or variants gracefully where reasonable.

The formatted/human-readable output (i.e. without `--json` flag) is **not** considered stable.
Please prefer the JSON output for scripts, since I reserve the right to make any changes to the human-readable output.

The `clarion-ipc` sub-crate (like other clarion sub-crates) is *not* API-stable in terms of the Rust semver; rather, it follows the version of clarion itself.
In particular, new struct fields and enum variants will be added.
