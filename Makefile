# Cordial developer shortcuts. `make dev` is the only one you need day to day.

.PHONY: dev ui test test-ui live build clean llama

dev: ## run the desktop app (builds llama.cpp on first run)
	cargo run -p cordial-app

ui: ## run the window in a browser against the demo bridge
	npm --prefix ui run dev

test: ## harness, prompt and tool tests (no model needed)
	cargo test --workspace --exclude cordial-app

test-ui: ## UI reducer, renderer and window tests
	npm --prefix ui run test

live: ## behavioural suite against the real weights
	cargo test -p cordial-core --test live_model -- --ignored --nocapture

ui-build:
	npm --prefix ui ci && npm --prefix ui run build

build: ui-build ## release bundle for this machine
	cargo build -p cordial-app --release

clean:
	cargo clean
	rm -rf ui/dist target/cordial-llama-cache

# Force a rebuild of the pinned llama.cpp (e.g. after editing CORDIAL_* flags).
llama:
	rm -rf target/cordial-llama-cache target/*/build/cordial-llama-* target/*/build/llama-build
