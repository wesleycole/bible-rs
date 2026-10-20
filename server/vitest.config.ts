import { defineConfig } from "vitest/config"

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    env: { LINGER_SERVER_SECRET: "test-secret" },
    server: { deps: { inline: ["convex-test", "@convex-dev/rate-limiter", "@convex-dev/workflow", "@convex-dev/workpool", "@convex-dev/batch-worker"] } },
    testTimeout: 30_000
  }
})
