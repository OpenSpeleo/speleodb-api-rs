.PHONY: build test clean setup update

build:
	cargo build --locked --all-features

test:
	cargo test --locked --all-targets --all-features
	cargo test --locked --doc
	cargo test --manifest-path xtask/Cargo.toml --locked

clean:
	cargo clean
	cargo clean --manifest-path xtask/Cargo.toml

setup:
	python3 scripts/patch-msquic.py
	curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
	# cargo install cargo-binstall --locked
	cargo binstall --locked --no-confirm prek
	cargo binstall --locked --no-confirm cargo-outdated
	cargo binstall --locked --no-confirm cargo-machete
	cargo binstall --locked --no-confirm cargo-deny
	cargo binstall --locked --no-confirm cargo-audit

update:
	cargo update --manifest-path Cargo.toml --recursive
	cargo update --manifest-path xtask/Cargo.toml --recursive
	cargo outdated --manifest-path Cargo.toml --depth 1
	cargo outdated --manifest-path xtask/Cargo.toml --depth 1
