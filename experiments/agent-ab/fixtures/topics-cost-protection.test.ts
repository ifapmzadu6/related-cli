import { describe, expect, it } from "vitest";
import { handleTopicsList } from "../src/topics";

type Limiter = { limit(options: { key: string }): Promise<{ success: boolean }> };

function fakeLimiter(success: boolean, calls: string[], error?: Error): Limiter {
  return {
    limit({ key }) {
      calls.push(key);
      if (error) return Promise.reject(error);
      return Promise.resolve({ success });
    },
  };
}

function fakeEnv(tokenLimiter: Limiter, ipLimiter: Limiter, points: unknown[]): never {
  const analytics = {
    writeDataPoint(point: unknown) {
      points.push(point);
    },
  };
  return {
    TOPICS_TOKEN_RATE_LIMITER: tokenLimiter,
    TOPICS_IP_RATE_LIMITER: ipLimiter,
    TOPICS_SECURITY_EVENTS: analytics,
    TOPICS_REJECTIONS: analytics,
    FCM_PROJECT_ID: "test",
    FCM_CLIENT_EMAIL: "test@example.com",
    FCM_PRIVATE_KEY: "test",
  } as never;
}

function request(token: string, ip = "203.0.113.50"): Request {
  return new Request("https://example.test/api/notification/topics", {
    method: "POST",
    headers: { "Content-Type": "application/json", "CF-Connecting-IP": ip },
    body: JSON.stringify({ fcmToken: token }),
  });
}

describe("topics cost protection acceptance", () => {
  it("hashes token keys and does not consume IP quota after token rejection", async () => {
    const token = "same-valid-format-token";
    const ip = "203.0.113.50";
    const tokenCalls: string[] = [];
    const ipCalls: string[] = [];
    const points: unknown[] = [];
    const response = await handleTopicsList(
      request(token, ip),
      fakeEnv(fakeLimiter(false, tokenCalls), fakeLimiter(true, ipCalls), points),
    );

    expect(response.status).toBe(429);
    expect(tokenCalls).toHaveLength(1);
    expect(tokenCalls[0]).toMatch(/^[0-9a-f]{64}$/);
    expect(ipCalls).toEqual([]);
    expect(JSON.stringify(points)).not.toContain(token);
    expect(JSON.stringify(points)).not.toContain(ip);
  });

  it("checks the shared IP quota after the token quota allows", async () => {
    const tokenCalls: string[] = [];
    const ipCalls: string[] = [];
    const points: unknown[] = [];
    const response = await handleTopicsList(
      request("new-valid-format-token", "203.0.113.60"),
      fakeEnv(fakeLimiter(true, tokenCalls), fakeLimiter(false, ipCalls), points),
    );

    expect(response.status).toBe(429);
    expect(tokenCalls).toHaveLength(1);
    expect(ipCalls).toHaveLength(1);
  });

  it("fails closed when a limiter is unavailable", async () => {
    const response = await handleTopicsList(
      request("valid-format-token"),
      fakeEnv(fakeLimiter(true, [], new Error("unavailable")), fakeLimiter(true, []), []),
    );
    expect(response.status).toBe(503);
  });

  it("rejects declared and streamed bodies larger than 8 KiB", async () => {
    const env = fakeEnv(fakeLimiter(true, []), fakeLimiter(true, []), []);
    const declared = new Request("https://example.test/api/notification/topics", {
      method: "POST",
      headers: { "Content-Length": "8193" },
      body: "{}",
    });
    expect((await handleTopicsList(declared, env)).status).toBe(413);

    const bytes = new TextEncoder().encode(JSON.stringify({ fcmToken: "あ".repeat(3_000) }));
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes);
        controller.close();
      },
    });
    const streamed = new Request("https://example.test/api/notification/topics", {
      method: "POST",
      body,
      duplex: "half",
    } as RequestInit & { duplex: "half" });
    expect((await handleTopicsList(streamed, env)).status).toBe(413);
  });
});
