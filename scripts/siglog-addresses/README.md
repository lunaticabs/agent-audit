# siglog-addresses

Standalone Rust workspace for extracting the address that appears immediately
after each `Sender : <address>` line in `sig0711.log`.

Short form from the repository root:

```sh
cargo xtask addrs
```

Direct form:

```sh
cargo run --manifest-path scripts/siglog-addresses/Cargo.toml
```

The default source is:

```text
http://65.109.115.133:4500/file/sig0711.log
```

The default output is:

```text
scripts/addresses/addrss.txt
```

Override either path when needed:

```sh
cargo run --manifest-path scripts/siglog-addresses/Cargo.toml -- \
  --source /tmp/sig0711.log \
  --output /tmp/addrs.txt
```

Short form with overrides:

```sh
cargo xtask addrs \
  --source /tmp/sig0711.log \
  --output /tmp/addrs.txt
```

Filter by `Sender` only when needed:

```sh
cargo xtask addrs --sender 0x9eD59587af8D7E156707539B9A4a22e7B3Cac1a0
```

Run tests:

```sh
cargo test --manifest-path scripts/siglog-addresses/Cargo.toml
```
