import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { clearSettings, GUEST_SETTINGS_ID, loadSettings, saveSettings } from "../src/lib/settings"
import { loadLocalVideoJobs, saveLocalVideoJobs, type VideoJob } from "../src/lib/video-gen"

const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage")
let values: Map<string, string>

beforeEach(() => {
  values = new Map()
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value) },
      removeItem: (key: string) => { values.delete(key) },
    },
  })
})

afterEach(() => {
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage)
  else Reflect.deleteProperty(globalThis, "localStorage")
})

describe("Yunova browser data compatibility", () => {
  test("retains existing user settings and migrates them on save", () => {
    values.set("novachat:upstream:v2:7", JSON.stringify({ model: "existing-model" }))
    const settings = loadSettings(7)
    expect(settings.model).toBe("existing-model")
    expect(loadSettings(8).model).not.toBe("existing-model")
    saveSettings(7, settings)
    expect(JSON.parse(values.get("yunova:upstream:v2:7")!).model).toBe("existing-model")
    expect(values.has("novachat:upstream:v2:7")).toBe(false)
  })

  test("prefers current settings and clears both names without restoring old values", () => {
    values.set("novachat:upstream:v2:7", JSON.stringify({ model: "old-model" }))
    values.set("yunova:upstream:v2:7", JSON.stringify({ model: "current-model" }))
    expect(loadSettings(7).model).toBe("current-model")
    clearSettings(7)
    expect(values.size).toBe(0)
    expect(loadSettings(7).model).not.toBe("old-model")
  })

  test("preserves guest isolation when loading legacy settings", () => {
    values.set(`novachat:upstream:v2:${GUEST_SETTINGS_ID}`, JSON.stringify({
      model: "guest-model", chatMode: "platform", imageMode: "platform", cloudSync: true,
    }))
    expect(loadSettings(GUEST_SETTINGS_ID)).toMatchObject({
      model: "guest-model", chatMode: "byok", imageMode: "byok", cloudSync: false,
    })
  })

  test("retains existing video jobs and does not resurrect cleared history", () => {
    const job: VideoJob = {
      token: "local-existing", model: "video-model", prompt: "ocean", seconds: 8,
      size: "1280x720", input_image_path: null, status: "pending", progress: 0,
      video_path: null, error: null, cost_credits: 0, refunded: false,
      created_at: "2026-09-09T00:00:00Z", finished_at: null, local: true,
      local_base_url: "https://video.example.com", local_upstream_id: "video-existing",
    }
    values.set("novachat:local-video-jobs:v1:7", JSON.stringify([job]))
    expect(loadLocalVideoJobs(7)).toEqual([job])
    expect(loadLocalVideoJobs(8)).toEqual([])
    saveLocalVideoJobs(7, loadLocalVideoJobs(7))
    expect(values.has("novachat:local-video-jobs:v1:7")).toBe(false)
    expect(loadLocalVideoJobs(7)).toEqual([job])
    saveLocalVideoJobs(7, [])
    expect(loadLocalVideoJobs(7)).toEqual([])
  })
})
