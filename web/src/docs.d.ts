// web/content/*.md, rendered at build time by docs.plugin.ts
declare module '*.md' {
  const doc: {
    title: string
    html: string
    sections: { id: string; title: string; part: string | null }[]
  }
  export default doc
}
