set shell := ["bash", "-uc"]

default:
    @just --list

build:
    cargo build

release:
    cargo build --release

run *args:
    cargo run -- {{args}}

check:
    cargo clippy
    cargo fmt --check

check-all:
    ./scripts/check-all

fmt:
    cargo fmt

clean:
    cargo clean

install: release
    @cp target/release/grimoire ~/.local/bin/grimoire
    @if [ "$(uname)" = "Darwin" ]; then codesign -s - ~/.local/bin/grimoire; fi
    @echo "Installed → ~/.local/bin/grimoire"
