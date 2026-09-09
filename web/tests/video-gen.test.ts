import { afterEach, describe, expect, test } from "bun:test"
import {
  createLocalVideoJob,
  getLocalVideoJob,
  listVideoModels,
  type VideoUpstreamConfig,
} from "../src/lib/video-gen"

const originalFetch = globalThis.fetch
const upstream: VideoUpstreamConfig = {
  baseUrl: "http://192.168.13.91:8000",
  apiKey: "local-key",
}

afterEach(() => {
  globalThis.fetch = originalFetch
})

describe("browser-local video API", () => {
  test("loads models directly from the configured local service", async () => {
    globalThis.fetch = (async (input, init) => {
      expect(String(input)).toBe("http://192.168.13.91:8000/v1/models")
      expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer local-key")
      expect(init?.credentials).toBe("omit")
      return Response.json({ data: [{ id: "wan2.1" }, { id: "wan2.2" }] })
    }) as typeof fetch

    const models = await listVideoModels(upstream)
    expect(models.map((model) => model.model)).toEqual(["wan2.1", "wan2.2"])
  })

  test("creates a multipart job without calling a Yunova API", async () => {
    globalThis.fetch = (async (input, init) => {
      expect(String(input)).toBe("http://192.168.13.91:8000/v1/videos")
      expect(init?.method).toBe("POST")
      const form = init?.body as FormData
      expect(form.get("model")).toBe("wan2.1")
      expect(form.get("prompt")).toBe("ocean")
      expect(form.get("seconds")).toBe("8")
      expect(form.get("size")).toBe("1280x720")
      return Response.json({ id: "video-123", status: "queued", progress: 2 })
    }) as typeof fetch

    const job = await createLocalVideoJob(
      { model: "wan2.1", prompt: "ocean", seconds: 8, size: "1280x720" },
      upstream
    )
    expect(job.local).toBe(true)
    expect(job.local_upstream_id).toBe("video-123")
    expect(job.status).toBe("pending")
    expect(job.progress).toBe(2)
  })

  test("polls the local service directly", async () => {
    globalThis.fetch = (async (input) => {
      expect(String(input)).toBe("http://192.168.13.91:8000/v1/videos/video-123")
      return Response.json({ id: "video-123", status: "in_progress", progress: 42 })
    }) as typeof fetch

    const job = await getLocalVideoJob(
      {
        token: "local-test",
        model: "wan2.1",
        prompt: "ocean",
        seconds: 8,
        size: "1280x720",
        input_image_path: null,
        status: "pending",
        progress: 0,
        video_path: null,
        error: null,
        cost_credits: 0,
        refunded: false,
        created_at: new Date().toISOString(),
        finished_at: null,
        local: true,
        local_base_url: upstream.baseUrl,
        local_upstream_id: "video-123",
      },
      upstream
    )
    expect(job.status).toBe("running")
    expect(job.progress).toBe(42)
  })

  test("downloads a completed video in the browser", async () => {
    const calls: string[] = []
    globalThis.fetch = (async (input) => {
      calls.push(String(input))
      if (calls.length === 1) {
        return Response.json({ id: "video-123", status: "completed", progress: 100 })
      }
      return new Response(new Blob(["video-bytes"], { type: "video/mp4" }))
    }) as typeof fetch

    const job = await getLocalVideoJob(
      {
        token: "local-test",
        model: "wan2.1",
        prompt: "ocean",
        seconds: 8,
        size: "1280x720",
        input_image_path: null,
        status: "running",
        progress: 80,
        video_path: null,
        error: null,
        cost_credits: 0,
        refunded: false,
        created_at: new Date().toISOString(),
        finished_at: null,
        local: true,
        local_base_url: upstream.baseUrl,
        local_upstream_id: "video-123",
      },
      upstream
    )
    expect(calls).toEqual([
      "http://192.168.13.91:8000/v1/videos/video-123",
      "http://192.168.13.91:8000/v1/videos/video-123/content",
    ])
    expect(job.status).toBe("completed")
    expect(job.video_path?.startsWith("blob:")).toBe(true)
    if (job.video_path) URL.revokeObjectURL(job.video_path)
  })

  test("turns browser network failures into an actionable error", async () => {
    globalThis.fetch = (async () => {
      throw new TypeError("Failed to fetch")
    }) as typeof fetch

    await expect(listVideoModels(upstream)).rejects.toThrow("CORS 和局域网访问")
  })
})
