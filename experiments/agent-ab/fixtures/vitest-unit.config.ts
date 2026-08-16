import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/topics.agent-ab.test.ts"],
    fileParallelism: false,
    sequence: { concurrent: false },
    testTimeout: 15_000,
  },
});
