# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Yunova is a self-hosted Agent platform combining multi-model chat, remote tool execution, media creation, and workflows. Its Rust/Axum backend serves a React SPA, proxies OpenAI / Anthropic / Gemini calls, and stores conversations, skills, prompts, images, and quota balances in a SQLite/MySQL/Postgres database. The frontend is embedded into the Rust binary at build time via `rust-embed`, so a single `cargo build` produces one shippable executable.

## Commands

Backend (repo root):
- `cargo run` — starts the server on `127.0.0.1:3000` (override with `YUNOVA_BIND`). On first run without a configured DB, the app exposes `/setup` and the frontend's SetupPage walks the user through picking SQLite/MySQL/Postgres.
- `cargo check` — fast type-check; the first run still invokes `bun install && bun run build` in `web/` via [build.rs](build.rs).
- Environment: `YUNOVA_DATA_DIR` (default `./data` — SQLite DB, local media, `yunova.toml` live here), `YUNOVA_DATABASE_URL` / `DATABASE_URL` (skips the install wizard), `YUNOVA_CONFIG` (path for the TOML config). Configure optional S3-compatible media storage in the admin UI; it persists to `[storage]` in `yunova.toml` and applies at runtime. Legacy `YUNOVA_STORAGE_BACKEND=s3` plus `YUNOVA_S3_*` variables remain fallback-only.

Frontend (in `web/`):
- `bun run dev` — Vite dev server (expects the Rust backend on :3000 for `/api` calls; configure proxy in `vite.config.ts` if needed).
- `bun run build` — `tsc -b && vite build`. Outputs to `web/dist/`, which `rust-embed` bundles at compile time.
- `bun run lint` — ESLint. Project-wide `react-hooks/set-state-in-effect` warnings exist in pre-existing dialogs; don't fix them reactively. `rules-of-hooks` errors are real — local helpers named `useFoo` inside a component get mistaken for hooks; rename to `handleFoo` / `applyFoo`.
- Run backend tests with `cargo test --workspace --locked` and frontend tests with `bun test` in `web/`. Also verify the built application over HTTP.

Docker:
- `docker compose up -d` — SQLite (walk through `/setup` on first boot).
- `docker compose --profile mysql up -d` / `--profile postgres up -d` — sets `YUNOVA_DATABASE_URL` so the DB step is skipped; you still create the first admin via `/setup`.

## Architecture

### Boot flow
[src/main.rs](src/main.rs) holds `AppState { installed: Arc<RwLock<Option<InstalledState>>>, http, config_path, data_dir, storage, config_lock }`. `storage` is a hot-swappable local/S3 media backend managed by the admin storage API; S3 reads fall back to legacy local objects. `config_lock` serializes admin writes to `yunova.toml`. `InstalledState { pool, kind }` is `None` until the setup wizard (or `YUNOVA_DATABASE_URL`) supplies a connection string. All protected routes go through `require_auth` middleware which loads `InstalledState` into request extensions alongside `CurrentUser { id }` — downstream handlers take `Extension<InstalledState>` and `Extension<CurrentUser>` rather than re-reading `AppState`.

### Database layer
Three dialects share one schema. Conventions in [src/db.rs](src/db.rs):
- **Migrations are numbered `NNNN_name.sql` per dialect** under `migrations/{sqlite,mysql,postgres}/`, compiled in via `include_str!` in three parallel arrays. **Adding a migration requires editing all three arrays and shipping all three files.** The runner applies unseen ids in order, splitting on `;` outside string/comment literals.
- `db::q(kind, sql)` rewrites `?` → `$1, $2, …` for Postgres; pass all SQL through it.
- `db::bool_true(kind)` → `"1"` or `"TRUE"` for `WHERE` clauses; `db::bool_as_int(kind, col)` → a `CASE` expression so booleans decode as `i64` across dialects. Postgres `BOOLEAN` would otherwise not decode into `i64` via sqlx-any.
- `db::now_expr(kind)` for `updated_at` defaults in `UPDATE` statements.
- Inserts returning the new id differ per dialect: Sqlite/Postgres use `RETURNING id`, MySQL uses `LAST_INSERT_ID()` inside a transaction. See [src/skills.rs](src/skills.rs#L213-L282) for the canonical pattern.
- Case-insensitive username lookup: `db::ci_eq(kind, "username")` — SQLite uses `COLLATE NOCASE` on the column, others wrap in `LOWER()`.

### Quota + billing system
[src/quota.rs](src/quota.rs) is the nerve center. Three tables (`app_settings` K/V, `user_balances`, `balance_ledger`) plus [src/usage.rs](src/usage.rs) for token extraction:
- **Quota is the site's own unit** — an integer rendered directly in the UI with no currency symbol. Model prices live in `model_pricing` as **micro-USD** (1 USD = 1_000_000) transcribed from each provider's published rate: `input_price` / `output_price` / `cached_input_price` per 1M tokens for chat, `per_call_price` for image, `base_price` + `per_second_price` for video. Two settings convert cost to quota: `quota_per_usd` (default 500000) and `price_multiplier_percent` (default 100). `QuotaRate::load` re-reads both on every billing decision so admin edits apply immediately.
- **Chat bills after the response.** `channels::authorize_chat` only checks the whitelist and that the balance is positive — it deducts nothing, because token counts aren't known until the upstream replies. `MeteredStream` in [src/main.rs](src/main.rs) forwards SSE bytes untouched while feeding them to `usage::UsageAccumulator`, buffering partial frames across chunk boundaries. Its `Drop` impl calls `channels::settle_chat`, so billing runs exactly once whether the stream ended cleanly, errored, or the client disconnected. **An upstream that reports no usage is never billed** — never guess a token count.
- **`quota::settle` may go negative** by at most one request (the tokens were already consumed upstream); the next `authorize_chat` then refuses until the user tops up. `quota::try_deduct` is the strict variant still used by image/video, which do pre-deduct and refund on failure because those APIs report no tokens.
- **Usage parsing is protocol-specific** ([src/usage.rs](src/usage.rs)): OpenAI Responses nests it under `response.usage`, Anthropic splits input across `message_start` and output across `message_delta`, Gemini uses `usageMetadata` and counts `thoughtsTokenCount` as output. The accumulator takes the max of each counter since every provider reports cumulative totals. Anthropic's `cache_read_input_tokens` are added back into `input` and marked as the cached subset so they bill at the discounted rate exactly once.
- **Invites** ([src/invites.rs](src/invites.rs)): every user has a 7-char code from a Crockford-ish alphabet (no 0/O/1/I/L). Generated lazily by `ensure_code` (race-safe via `UPDATE … WHERE invite_code IS NULL`). Successful referral at registration grants both parties via `quota::grant`, writing reasons like `invite_reward_inviter:<username>` to the ledger.

### Route composition
Everything mounts under `/api` in [src/main.rs](src/main.rs) `build_router`. Public routes (health, auth, setup). Protected routes (everything else) sit behind `require_auth`; admin-only endpoints compose an additional `admin::require_admin` middleware. When adding a feature module, it exposes `pub fn routes() -> Router<AppState>` and `main.rs` `.merge(...)`'s it in.

### Frontend
React 19 + Vite 8 + Tailwind 4 + shadcn/ui (under `web/src/components/ui/`). Single auth context ([web/src/lib/auth-context.tsx](web/src/lib/auth-context.tsx)) switches between `loading`/`setup`/`anon`/`authed` states on boot, driven by `GET /api/setup/status` then `/api/auth/me`.

- **ChatPage** is the big one — ~1000 lines, handles both chat and image modes, skill attachment, image plaza publish overlays, quota badge, shared-backend fallback UI.
- Per-feature API clients live in `web/src/lib/<feature>.ts`. Upstream proxy helpers (`chat-stream.ts`, `image-gen.ts`, `models.ts`) always send `X-Upstream-Url/Key` headers; leaving them empty is how the backend knows to use shared credentials.
- **Settings are dual-storage**: localStorage by default, with optional cloud sync when `settings.cloudSync` is set (server stores in `user_settings` table, never returns keys to other clients). See [web/src/lib/settings.ts](web/src/lib/settings.ts) `loadEffectiveSettings`.
- The UI is Chinese throughout — labels, error messages, admin panels. Match that when adding user-facing strings.

### Adding a new feature

For a typical "public + private library" feature (mirroring how skills/prompts/plaza images are organized):
1. Write three migrations (`NNNN_name.sql` in each dialect dir) and register them in the three arrays of `db.rs`.
2. Create `src/<feature>.rs` with a `pub fn routes() -> Router<AppState>` plus CRUD handlers using `Extension<InstalledState>` / `Extension<CurrentUser>`. Copy the insert-with-id pattern and `db::q` / `db::bool_as_int` usage from [src/skills.rs](src/skills.rs).
3. Add `mod <feature>;` + `.merge(<feature>::routes())` in `main.rs`.
4. Create `web/src/lib/<feature>.ts` as the API client, and component(s) under `web/src/components/app/`.
5. Build both sides (`cargo check` and `bun run build`) before considering it done.

## Gotchas

- **build.rs runs bun on every `cargo check`.** If `web/node_modules` is missing it runs `bun install` first — can add minutes to a cold build. The compiled `web/dist/` is what gets embedded; deleting it and forgetting to rebuild the frontend will leave stale assets inside the Rust binary.
- **Windows CRLF noise**: git will emit `LF will be replaced by CRLF` warnings on commit. They're harmless; don't rewrite files to silence them.
- **Concurrent linter edits**: this repo's workflow sometimes produces `<system-reminder>` notes saying another agent/linter modified a file. When you see an in-flight change to `db.rs` (new migration ids) or `image_plaza.rs` (new columns), assume it's intentional and work around it — don't revert unless the user asks.
- **Keys never come back**: admin `GET /api/admin/app-settings` returns `*_key_set: bool` instead of the actual value. Frontend treats empty input as "don't change" and the literal string `"-"` as "clear". Preserve this contract.
- **Prices change over time**: always re-read `model_pricing` and `QuotaRate` at the moment you bill or refund; don't cache them across a request boundary. Image/video refunds must use the amount actually deducted, not a fresh lookup.
- **Never bill a chat call without upstream usage.** If `UsageAccumulator` comes back empty, the request is free — inventing an estimate would silently overcharge users on every provider that omits usage frames.
