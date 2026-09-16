/**
 * Desktop client downloads.
 *
 * Binaries are published as GitHub Release assets rather than served by this
 * server: the server image has no reason to carry five platform builds, and a
 * self-hosted instance should not have to mirror them to let a user install
 * the client.
 *
 * The listing is therefore fetched from the public GitHub API at view time.
 * That call can fail — no egress, rate limit, an air-gapped deployment — so
 * every platform keeps a static fallback link to the "latest release" page,
 * which resolves server-side at GitHub and never goes stale.
 */

export const DESKTOP_REPO = "yiranxiaohui/Yunova"

export const RELEASES_URL = `https://github.com/${DESKTOP_REPO}/releases/latest`

export type DesktopOs = "macos" | "windows" | "linux"

export interface DesktopBuild {
  /** Stable key, also the suffix of the published asset name. */
  target: string
  os: DesktopOs
  /** Human label for the CPU family, e.g. "Apple 芯片". */
  arch: string
  /** Archive extension used by the release workflow for this target. */
  ext: "tar.gz" | "zip"
}

export const DESKTOP_BUILDS: DesktopBuild[] = [
  { target: "aarch64-apple-darwin", os: "macos", arch: "Apple 芯片", ext: "tar.gz" },
  { target: "x86_64-apple-darwin", os: "macos", arch: "Intel", ext: "tar.gz" },
  { target: "x86_64-pc-windows-msvc", os: "windows", arch: "x64", ext: "zip" },
  { target: "x86_64-unknown-linux-musl", os: "linux", arch: "x64", ext: "tar.gz" },
  { target: "aarch64-unknown-linux-musl", os: "linux", arch: "ARM64", ext: "tar.gz" },
]

export const OS_LABEL: Record<DesktopOs, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "Linux",
}

/** Asset name produced by the release workflow. */
export function assetName(b: DesktopBuild): string {
  return `yunova-desktop-${b.target}.${b.ext}`
}

export interface ReleaseInfo {
  tag: string
  /** Asset name → browser download URL, for the assets that exist. */
  assets: Record<string, string>
  publishedAt: string | null
}

interface GhRelease {
  tag_name?: string
  published_at?: string | null
  assets?: Array<{ name?: string; browser_download_url?: string }>
}

/** Fetch the latest published release, or `null` when GitHub is unreachable. */
export async function fetchLatestRelease(
  signal?: AbortSignal
): Promise<ReleaseInfo | null> {
  try {
    const res = await fetch(
      `https://api.github.com/repos/${DESKTOP_REPO}/releases/latest`,
      { signal, headers: { Accept: "application/vnd.github+json" } }
    )
    if (!res.ok) return null
    const body = (await res.json()) as GhRelease
    if (!body.tag_name) return null
    const assets: Record<string, string> = {}
    for (const a of body.assets ?? []) {
      if (a.name && a.browser_download_url) assets[a.name] = a.browser_download_url
    }
    return {
      tag: body.tag_name,
      assets,
      publishedAt: body.published_at ?? null,
    }
  } catch {
    return null
  }
}

/** Direct asset URL when known, otherwise the releases page. */
export function downloadUrl(b: DesktopBuild, release: ReleaseInfo | null): string {
  return release?.assets[assetName(b)] ?? RELEASES_URL
}

/** Best guess at the visitor's platform, used to promote one button. */
export function guessOs(ua = navigator.userAgent): DesktopOs | null {
  const s = ua.toLowerCase()
  // Phones and tablets are remote controls, not execution targets. They are
  // checked first because both lie about the desktop OS: iOS says "like Mac
  // OS X" and Android says "linux".
  if (s.includes("iphone") || s.includes("ipad") || s.includes("android")) return null
  if (s.includes("windows")) return "windows"
  if (s.includes("mac os") || s.includes("macintosh")) return "macos"
  if (s.includes("linux")) return "linux"
  return null
}

/**
 * The build to offer first.
 *
 * A browser cannot read the CPU family, and `navigator.platform` reports
 * "MacIntel" even on Apple silicon under Rosetta-era shims, so the arch is a
 * guess: prefer Apple silicon on modern macOS and let the user pick the other
 * entry from the full list when the guess is wrong.
 */
export function preferredBuild(os: DesktopOs | null): DesktopBuild | null {
  if (!os) return null
  if (os === "macos") {
    return DESKTOP_BUILDS.find((b) => b.target === "aarch64-apple-darwin") ?? null
  }
  if (os === "windows") {
    return DESKTOP_BUILDS.find((b) => b.target === "x86_64-pc-windows-msvc") ?? null
  }
  return DESKTOP_BUILDS.find((b) => b.target === "x86_64-unknown-linux-musl") ?? null
}
