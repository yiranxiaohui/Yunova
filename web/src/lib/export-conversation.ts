import type { StoredMessage, Conversation } from "./conversations"

export type ExportFormat = "markdown" | "json"

export type ExportInput = {
  conversation: Pick<Conversation, "id" | "title" | "system_prompt" | "created_at" | "updated_at">
  messages: StoredMessage[]
}

function safeFilename(stem: string, ext: string): string {
  const cleaned = stem
    .replace(/[\\/:*?"<>|\n\r\t]+/g, "")
    .trim()
    .slice(0, 80)
  return `${cleaned || "对话"}.${ext}`
}

/** Convert a conversation to a Markdown document. Each message becomes a
 *  `## 用户` / `## 助手` heading with the body preserved verbatim. Image
 *  references stay as the original `![](/api/images/…)` markdown so they
 *  resolve when opened on the same host; in offline / cross-host viewers
 *  they'll show as broken links, which is acceptable for a portable export. */
function toMarkdown(input: ExportInput): string {
  const { conversation, messages } = input
  const lines: string[] = []
  lines.push(`# ${conversation.title || "对话"}`)
  lines.push("")
  lines.push(
    `_导出时间: ${new Date().toLocaleString()} · 共 ${messages.length} 条消息_`
  )
  lines.push("")
  if (conversation.system_prompt?.trim()) {
    lines.push("## 系统提示词")
    lines.push("")
    lines.push("> " + conversation.system_prompt.replace(/\n/g, "\n> "))
    lines.push("")
  }
  for (const m of messages) {
    const heading =
      m.role === "user" ? "## 用户" : m.role === "assistant" ? "## 助手" : "## 系统"
    lines.push(heading)
    lines.push("")
    lines.push(m.content)
    lines.push("")
  }
  return lines.join("\n")
}

function toJson(input: ExportInput): string {
  return JSON.stringify(
    {
      yunova_export_version: 1,
      exported_at: new Date().toISOString(),
      conversation: {
        id: input.conversation.id,
        title: input.conversation.title,
        system_prompt: input.conversation.system_prompt,
        created_at: input.conversation.created_at,
        updated_at: input.conversation.updated_at,
      },
      messages: input.messages.map((m) => ({
        id: m.id,
        role: m.role,
        content: m.content,
        created_at: m.created_at,
      })),
    },
    null,
    2
  )
}

/** Build a Blob in the requested format. Caller is responsible for triggering
 *  the download. */
export function buildExportBlob(
  input: ExportInput,
  format: ExportFormat
): { blob: Blob; filename: string } {
  if (format === "json") {
    return {
      blob: new Blob([toJson(input)], { type: "application/json" }),
      filename: safeFilename(input.conversation.title, "json"),
    }
  }
  return {
    blob: new Blob([toMarkdown(input)], { type: "text/markdown;charset=utf-8" }),
    filename: safeFilename(input.conversation.title, "md"),
  }
}

/** Wires together blob construction + an ephemeral <a download> click. */
export function downloadConversation(
  input: ExportInput,
  format: ExportFormat
): void {
  const { blob, filename } = buildExportBlob(input, format)
  const url = URL.createObjectURL(blob)
  const a = document.createElement("a")
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  setTimeout(() => URL.revokeObjectURL(url), 5000)
}
