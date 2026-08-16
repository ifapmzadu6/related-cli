# Parser fuzzing

The fuzz target feeds arbitrary bytes into the custom Git log, pack object,
delta, commit, tree, and pack-index parsers.

Install `cargo-fuzz` and run the target with a nightly toolchain:

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run repository_parsers
```

Use a bounded smoke run before release-oriented changes to the parsers:

```sh
cargo +nightly fuzz run repository_parsers -- -max_total_time=60
```
