// Markdown for operator text (announcements): CommonMark with single newlines kept as line breaks, no raw HTML
// (it is escaped), links checked by markdown-it (no javascript:) and opened in a new tab. Loaded on first use,
// so the parser stays out of the main bundle.
import type MarkdownIt from 'markdown-it'

let md: Promise<MarkdownIt> | null = null

function load(): Promise<MarkdownIt> {
  md ??= import('markdown-it').then(({ default: MD }) => {
    const m = new MD({ html: false, linkify: true, breaks: true, typographer: false })
    const open = m.renderer.rules.link_open ?? ((t, i, o, _e, self) => self.renderToken(t, i, o))
    m.renderer.rules.link_open = (tokens, i, opts, env, self) => {
      tokens[i].attrSet('target', '_blank')
      tokens[i].attrSet('rel', 'noopener noreferrer')
      return open(tokens, i, opts, env, self)
    }
    return m
  })
  return md
}

export async function renderMarkdown(src: string): Promise<string> {
  return (await load()).render(src)
}
