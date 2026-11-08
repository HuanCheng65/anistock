// The reading documents (规则, 开发者说明, 指数编制方法): one Markdown file each in web/content/, the single source for
// both readers. Pages import the file and get HTML rendered here at build time (no parser in the browser); the same
// file is published unchanged at its page's path + ".md" (/rules -> /rules.md) and listed in /llms.txt, for agents
// and fetch tools that cannot run the app.
//
// Conventions in the Markdown (plain CommonMark otherwise, raw HTML off):
//   # Title                the document title (the page header shows it: left out of the HTML)
//   text before the first section heading: the lede
//   ## Part                 only in documents with parts (规则): a part label, and a group in the contents
//   ##/### Section {#id}    a section with a fixed anchor (other pages link to /rules#vcm)
//   one level below         a sub-heading inside the section
//   ```formula              a formula, lines indented by two spaces continue the line above
// Table cells that hold only a number, time or rate are set in the numeral font.
import { readFileSync, readdirSync } from 'node:fs'
import MarkdownIt from 'markdown-it'
import type { Plugin } from 'vite'

type Token = ReturnType<MarkdownIt['parse']>[number]

export interface DocSection { id: string; title: string; part: string | null }
export interface Doc { title: string; html: string; sections: DocSection[] }

const CONTENT = new URL('./content/', import.meta.url)

/** page path -> source file; the published copy is the page path + ".md" */
export const PUBLISHED: Record<string, string> = {
  '/rules': 'rules.md',
  '/rules/api': 'api.md',
  '/rules/heat-index': 'heat-index.md',
}

const md = new MarkdownIt({ html: false, linkify: false, typographer: false })
md.renderer.rules.fence = (tokens, i) => {
  const t = tokens[i]
  const body = t.content.replace(/\n$/, '')
  if (t.info.trim() === 'formula') {
    const lines = body.split('\n').map((l) => `<span${/^\s/.test(l) ? ' class="in"' : ''}>${md.utils.escapeHtml(l.trim())}</span>`)
    return `<p class="formula num">${lines.join('')}</p>\n`
  }
  return `<pre class="code"><code>${md.utils.escapeHtml(body)}</code></pre>\n`
}
const linkOpen = md.renderer.rules.link_open ?? ((t, i, o, _e, self) => self.renderToken(t, i, o))
md.renderer.rules.link_open = (tokens, i, opts, env, self) => {
  if (/^https?:/.test(tokens[i].attrGet('href') ?? '')) {
    tokens[i].attrSet('target', '_blank')
    tokens[i].attrSet('rel', 'noopener')
  }
  return linkOpen(tokens, i, opts, env, self)
}

const NUMERIC = /^[−+-]?[\d,.:]+\s*(%|倍|天|周|分钟|笔|股|星辉)?(\s*(–|至次日|至)\s*[\d,.:]+)?$/

/** "冷静期 {#vcm}" -> ["冷静期", "vcm"]; strips the marker from the inline token */
function anchor(inline: Token): [string, string | null] {
  const m = /\s*\{#([a-z0-9-]+)\}\s*$/.exec(inline.content)
  if (!m) return [inline.content, null]
  inline.content = inline.content.slice(0, m.index)
  const last = inline.children?.[inline.children.length - 1]
  if (last?.type === 'text') last.content = last.content.replace(/\s*\{#[a-z0-9-]+\}\s*$/, '')
  return [inline.content, m[1]]
}

export function renderDoc(src: string, file: string): Doc {
  const env = {}
  const tokens = md.parse(src, env)
  const hasParts = tokens.some((t, i) => t.type === 'heading_open' && t.tag === 'h2' && !/\{#/.test(tokens[i + 1].content))
  const secTag = hasParts ? 'h3' : 'h2'
  const subTag = hasParts ? 'h4' : 'h3'

  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]
    if (t.type === 'table_open') t.attrJoin('class', 't')
    if (t.type === 'td_open' && NUMERIC.test(tokens[i + 1].content.trim())) t.attrJoin('class', 'n')
  }

  let title = ''
  const sections: DocSection[] = []
  let part: string | null = null
  let html = ''
  let chunk: Token[] = []
  let open: 'lede' | 'sec' | null = null
  const flush = () => {
    if (chunk.length) html += md.renderer.render(chunk, md.options, env)
    chunk = []
  }
  const close = () => {
    flush()
    if (open) html += open === 'lede' ? '</div>\n' : '</section>\n'
    open = null
  }

  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i]
    if (t.type === 'heading_open') {
      const inline = tokens[i + 1]
      if (t.tag === 'h1') {
        title = inline.content
        i += 2
        continue
      }
      if (hasParts && t.tag === 'h2') {
        close()
        part = inline.content
        html += `<div class="part">${md.utils.escapeHtml(part)}</div>\n`
        i += 2
        continue
      }
      if (t.tag === secTag) {
        close()
        const [text, id] = anchor(inline)
        if (!id) throw new Error(`${file}: section "${text}" has no {#id}`)
        sections.push({ id, title: text, part })
        html += `<section id="${id}" class="sec">\n`
        open = 'sec'
        t.tag = 'h2'
        tokens[i + 2].tag = 'h2'
      } else if (t.tag === subTag) {
        t.tag = 'h3'
        tokens[i + 2].tag = 'h3'
      }
    } else if (!open) {
      html += '<div class="lede">\n'
      open = 'lede'
    }
    chunk.push(t)
  }
  close()
  if (!title) throw new Error(`${file}: no "# title"`)
  return { title, html, sections }
}

const read = (file: string) => readFileSync(new URL(file, CONTENT), 'utf8')

export function docs(): Plugin {
  return {
    name: 'aniex-docs',
    load(id) {
      const path = id.split('?')[0]
      if (!path.endsWith('.md') || !path.startsWith(CONTENT.pathname)) return null
      const file = path.slice(CONTENT.pathname.length)
      return `export default ${JSON.stringify(renderDoc(readFileSync(path, 'utf8'), file))}`
    },
    // the published copies, and the dev server answering the same paths
    generateBundle() {
      for (const [page, file] of Object.entries(PUBLISHED)) {
        this.emitFile({ type: 'asset', fileName: `${page.slice(1)}.md`, source: read(file) })
      }
      this.emitFile({ type: 'asset', fileName: 'llms.txt', source: read('llms.txt') })
    },
    configureServer(server) {
      for (const f of readdirSync(CONTENT)) server.watcher.add(new URL(f, CONTENT).pathname)
      server.middlewares.use((req, res, next) => {
        const url = (req.url ?? '').split('?')[0]
        const file = url === '/llms.txt' ? 'llms.txt' : url.endsWith('.md') ? PUBLISHED[url.slice(0, -3)] : undefined
        if (!file) return next()
        res.setHeader('content-type', 'text/plain; charset=utf-8')
        res.end(read(file))
      })
    },
  }
}
