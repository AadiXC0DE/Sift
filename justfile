default: dev

dev:
  pnpm tauri dev

dev-vite:
  pnpm dev:vite

test:
  pnpm test
  cargo test --manifest-path src-tauri/Cargo.toml

lint:
  pnpm lint
  cargo fmt --check --manifest-path src-tauri/Cargo.toml
  cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings

build:
  pnpm build
  pnpm tauri build

build-debug:
  pnpm tauri build --debug

bench:
  ./scripts/bench-start.sh

bench-start:
  ./scripts/bench-start.sh

bench-mem:
  ./scripts/bench-mem.sh

screenshots:
  pnpm tsx scripts/screenshots.ts

check-dto:
  pnpm tsx scripts/check-dto-sync.ts
