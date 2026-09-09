# Remove Legacy `shared_*` Upstream Plumbing

> Cleanup follow-up to the channels migration (Task 5 of `2026-05-18-multi-channel-pricing.md`).
> Legacy `shared_*` settings are no longer required — channels covers everything.
> Goal: delete all shared-upstream code paths, settings, UI, and headers.

## Scope summary

| Layer | Files |
|---|---|
| Backend resolver | `src/images.rs`, `src/studio.rs`, `src/main.rs` (proxy_get_forward), `src/channels.rs` (resolve_route header arm) |
| Backend admin | `src/credits.rs` (read_shared, SharedUpstream, SharedFlavor, get_shared_status, AdminSettingsView/Update shared_* fields, admin_list_upstream_models route) |
| Backend net_guard | `src/net_guard.rs` (used_shared param → drop) |
| Backend seed | `src/channels.rs::seed_from_legacy` + `src/main.rs:1056` call |
| Frontend api wrappers | `web/src/api/chat-stream.ts`, `web/src/api/studio.ts`, `web/src/api/models.ts`, `web/src/api/image-gen.ts` (~10 files using `X-Use-Shared`) |
| Frontend settings | `web/src/store/settings.ts` (`useShared` field), `web/src/components/SettingsDialog.tsx` (toggle) |
| Frontend admin | `web/src/pages/AdminPage.tsx` (shared upstream form section) |
| DB | `app_settings` rows for `shared_*` — leave in place (no-op reads after code removal), drop in optional follow-up migration |

## Phasing

### Phase A — backend resolver migration (P0, blocking)

Goal: nothing reads `shared_*` settings any more.

1. **images.rs**: rewrite `resolve_image_upstream` to use `channels::resolve_route(headers, "image", &client_model)`. Returned `Route` enum: `Byok` → header path; `Channels { chain }` → pick first matching protocol. Drop `used_shared` flag from return tuple → propagate "is BYOK" instead (= negation).
   - Replace every `used_shared` site (≈40 in this file) with `is_channel_route` (clearer name).
   - `image_jobs.used_shared` DB column: keep schema, write `1` for channel-route, `0` for BYOK (preserves history semantics).
2. **studio.rs**: same treatment for `resolve_openai_image_upstream` + `list_models`. `list_models` already routes via channels when no BYOK header — remove the `read_shared` branch entirely, fall through to channels fallback.
3. **net_guard.rs**: rename `used_shared` param → `is_channel` (semantics: trust net policy when we own the URL). Same callers.
4. **main.rs::proxy_get_forward** (L780-880): collapse `want_shared` flag — it's only used for error message differentiation. Drop the header read, simplify error text.
5. **channels.rs::resolve_route** (L723): drop `want_shared` header check — BYOK is detected by presence of `X-Upstream-Url` + `X-Upstream-Key`, no opt-out header needed.

Commit: `refactor(backend): route images/studio through channels, drop shared_* reads`

Verify: `cargo check` clean. Prod parity smoke (chat + image generate + studio).

### Phase B — backend admin/settings cleanup (P1)

Goal: delete dead code now that nothing reads it.

1. Delete `credits::read_shared`, `SharedUpstream`, `SharedFlavor` (pub).
2. Delete `credits::get_shared_status` + route `/api/credits/shared/status`.
3. Delete `credits::admin_list_upstream_models` + route `/api/admin/upstream/models`.
4. Strip `shared_enabled` + 10× `shared_*_url/key/model` fields from `AdminSettingsView` and `AdminSettingsUpdate`.
5. Delete `channels::seed_from_legacy` + the `main.rs:1056` call.
6. Audit: `rg 'shared_' src/` should only hit comments + the `used_shared` DB column (kept for history).

Commit: `refactor(backend): remove shared_* admin settings + helpers`

Verify: `cargo check`. Boot prod-image locally against staging DB; admin GET/PUT settings still works.

### Phase C — frontend API cleanup (P2a)

Goal: drop `X-Use-Shared` from all client requests.

1. `web/src/api/chat-stream.ts:292-300` — strip header.
2. `web/src/api/studio.ts:42-64` — strip header.
3. `web/src/api/models.ts:7-12` — strip header.
4. `web/src/api/image-gen.ts:285-329` — strip header.
5. Remaining 6 files: bulk `rg -l X-Use-Shared` sweep.

The backend after Phase A already ignores `X-Use-Shared`; this just trims dead bytes.

Commit: `refactor(web): stop sending X-Use-Shared header`

### Phase D — frontend settings + admin UI (P2b)

Goal: user-visible cleanup.

1. `web/src/store/settings.ts`: remove `useShared` field, migrate any reads.
2. `web/src/components/SettingsDialog.tsx`: delete "使用站点共享后端" toggle + sub-panels for shared upstream URL/key (these were BYOK-only fields disguised).
3. `web/src/pages/AdminPage.tsx`: delete "共享上游" form section (5 URL + 5 key + 5 model + 1 enable toggle ≈ 200 LOC).
4. Audit: `rg 'useShared|shared_enabled|shared_chat_|shared_image_' web/src/` should be empty.

Commit: `refactor(web): remove shared upstream UI from settings + admin`

### Phase E — verification + deploy

1. `cargo check` (already clean per phase) + `cd web && bun run typecheck`.
2. `cd web && bun run build` succeeds.
3. `git push origin main` → GHA → SSH prod pull + `up -d yunova`.
4. Manual smoke (prod):
   - Anonymous chat (channel route) ✅
   - Image generate via studio ✅
   - Admin page loads without errors ✅
   - Settings dialog loads without errors ✅

### Phase F (optional, deferred) — DB cleanup migration

Triple migration deleting `shared_*` rows from `app_settings`. Safe because:
- `seed_from_legacy` is deleted (no re-seed on next boot)
- No code reads these rows

Format: `migrations/{sqlite,postgres,mysql}/00NN_drop_shared_settings.sql`:
```sql
DELETE FROM app_settings WHERE setting_key LIKE 'shared\_%' ESCAPE '\';
```

NOT in scope of this branch — file as a follow-up issue.

## Risks

- Phase A `images.rs` has ~40 `used_shared` references with retry/refund branches — easy to miss one and double-refund or skip-deduct. Mitigation: rename param consistently (`is_channel_route`), re-read after patch.
- `image_jobs.used_shared` DB column: keep writing `1` for channel routes (was the historical value), don't break history queries. Don't alter column.
- Frontend dialog has localStorage migration concerns — bump store version + drop `useShared` from persisted state.
