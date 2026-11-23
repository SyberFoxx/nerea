# nerea

Linux-first AI desktop companion. `nerea-core` holds the UI-agnostic logic;
the UI shell (Tauri) plugs into it later.

```
crates/nerea-core   providers, secrets, conversation contract (no UI, no tokio)
crates/nerea-cli    headless harness: key set / providers / chat
```

## Try it

```bash
cargo run -p nerea-cli -- providers
cargo run -p nerea-cli -- key set opencode-go     # or export OPENCODE_API_KEY
cargo run -p nerea-cli -- chat opencode-go hola
cargo run -p nerea-cli -- chat opencode-go --stream contame algo
```
