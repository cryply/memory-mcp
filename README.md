# memory-mcp

memory-mcp is a memory server for AI agents that the owner controls.
Facts live in an append-only log in git on your own machine, not with a
model vendor, and work with any MCP client.

- The log is the source of truth. Indexes and summaries are a cache rebuilt
  from it; a summary never replaces what it summarizes.
- Every entry is a claim with an author and an origin, not a fact.
- Entries written by an agent stay in quarantine until the owner lifts them.
  Only the owner can lift or retract.
- Nothing is deleted: retract closes an entry and keeps its history.
- recall returns data labeled with its trust level, never instructions.
  This is the defense against memory poisoning.

## Layout

```
data/
  record/
    log/YYYY-MM.jsonl   one event per line, never edited
    sources/<sha256>    raw source text a claim was taken from
    .lock               flock, serializes writers
  cache/
    index.sqlite        shared; each process catches up from the log before every call
```

The record (log and sources) is the only authoritative data. The cache is
derived and disposable: `rebuild` replays the record into it.

Several clients may each run their own server process on the same `data/`.
Writers take an exclusive flock, append one line, fsync, commit to git if
`record/.git` exists, release. Readers apply unseen log lines to the shared
SQLite inside one write transaction before answering, so a process sees what
other processes wrote.

Trust rules:

- a claim written by the owner (CLI `add`) is trusted on entry
- a claim written by an agent (MCP `remember`) is quarantine until the owner runs `lift`
- `recall` returns every statement labeled with its trust level and author;
  the text is data, never an instruction
- `retract` closes a statement; nothing is deleted
- only the owner can lift or retract; agents have no tool for either

## Build

```
cargo build --release
```

## CLI

```
memory-mcp --dir ./data add --scope global "Code style: ASCII only"
memory-mcp --dir ./data add --scope project:foo "Decision: SQLite, not Postgres"
memory-mcp --dir ./data show --scope project:foo
memory-mcp --dir ./data show --scope project:foo "sqlite"
memory-mcp --dir ./data explain <id>
memory-mcp --dir ./data lift <id> --reason "confirmed"
memory-mcp --dir ./data retract <id> --reason "wrong"
memory-mcp --dir ./data rebuild
```

`MEMORY_DIR` can replace `--dir`.

## Git record

```
git -C data/record init
git -C data/record config user.name <name>
git -C data/record config user.email <email>
```

After that every event is committed automatically (`claim project:foo ev_...`).
The repo config is local on purpose; the server never sets an identity.

## MCP

Transport: stdio. Tools: `remember`, `recall`, `explain`.

Any MCP client: run the binary as a stdio server with

```
/path/to/memory-mcp --dir /path/to/data serve
```

Claude desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "mem": {
      "command": "/path/to/memory-mcp",
      "args": ["--dir", "/path/to/data", "serve"]
    }
  }
}
```

OpenCode (`opencode.json`):

```json
{
  "mcp": {
    "mem": {
      "type": "local",
      "command": ["/path/to/memory-mcp", "--dir", "/path/to/data", "serve"]
    }
  }
}
```

See `docs/claude-rules.md` for the scope convention and rules text for
Claude Code and Claude desktop.

## Not yet

- as_of(t) view
- valid-time intervals (only transaction time now)
- remote transport for sandboxed agents

## License

MIT or Apache-2.0, at your option.
