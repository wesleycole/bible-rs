# server/

- Before writing or reviewing Effect code, read `node_modules/effect/AGENTS.md` completely and follow
  its links. Confirm APIs in `node_modules/effect/ai-docs/` and `node_modules/effect/src/`: this is
  Effect 4, pinned exactly; don't write it from v3 memory.
- Every vendor is a `Context.Service` in `src/services/` with a live layer; tests swap in fakes
  through `test/harness.ts` and run the real Convex functions with convex-test.
- Convex functions the proxy calls take the server secret (`convex/lib/server.ts`); keep it that way.
- Before finishing: `pnpm typecheck && pnpm test`.
