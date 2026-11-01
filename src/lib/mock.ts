// Stand-in data until the SQLite cache exists. Delete this file when it does.
import type { Article, Feed, Folder } from './types'

export const folders: Folder[] = [
  { id: 'tech', title: 'Technology' },
  { id: 'design', title: 'Design' },
  { id: 'news', title: 'News' },
]

export const feeds: Feed[] = [
  { id: 'kernel', title: 'Kernel Notes', folderId: 'tech' },
  { id: 'rustacean', title: 'The Borrowed Reference', folderId: 'tech' },
  { id: 'webplat', title: 'Web Platform Weekly', folderId: 'tech' },
  { id: 'grid', title: 'On the Grid', folderId: 'design' },
  { id: 'type', title: 'Letterform', folderId: 'design' },
  { id: 'north', title: 'Northern Dispatch', folderId: 'news' },
  { id: 'harbor', title: 'Harbor Times', folderId: 'news' },
  { id: 'slow', title: 'Slow Mornings', folderId: null },
  { id: 'field', title: 'Field Recordings', folderId: null },
]

const ARTICLE_COUNT = 5000
const DAY = 24 * 60 * 60 * 1000

const subjects = [
  'Scheduler', 'Font rendering', 'Memory allocator', 'Color system', 'Harbor expansion',
  'Spring migration', 'Type scale', 'Build cache', 'City council', 'Night train',
  'Async runtime', 'Grid layout', 'Coastal weather', 'Sourdough starter', 'Field recorder',
]
const verbs = [
  'gets a rewrite', 'explained in ten minutes', 'and why it matters', 'is faster than expected',
  'after one year', 'the quiet way', 'meets the real world', 'without the hype',
  'under pressure', 'from first principles',
]
const authors = ['Ada Lind', 'Johan Berg', 'Mira Holm', 'Sam Ekström', 'Noor Vik', 'Leo Strand']
const sentences = [
  'The first draft was simple, and that turned out to be the hard part.',
  'Most of the work happened in places nobody was looking.',
  'Measurements told a different story than the benchmarks did.',
  'It is easy to underestimate how much a small default shapes behavior.',
  'Nothing here is new, but the combination is worth a closer look.',
  'The trade-off is real, and it is better to make it on purpose.',
  'Several readers wrote in with corrections, which are included below.',
  'A week of testing on older hardware changed the plan.',
]

function rng(seed: number): () => number {
  // Mulberry32: small, deterministic, good enough for fixtures.
  return () => {
    seed = (seed + 0x6d2b79f5) | 0
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function pick<T>(r: () => number, list: T[]): T {
  return list[Math.floor(r() * list.length)]
}

function paragraph(r: () => number): string {
  return Array.from({ length: 3 + Math.floor(r() * 3) }, () => pick(r, sentences)).join(' ')
}

function body(r: () => number, lead: string): string {
  return [
    `<p>${lead}</p>`,
    `<p>${paragraph(r)}</p>`,
    `<blockquote><p>${pick(r, sentences)}</p></blockquote>`,
    `<p>${paragraph(r)}</p>`,
    `<ul><li>${pick(r, sentences)}</li><li>${pick(r, sentences)}</li></ul>`,
    `<p>${paragraph(r)} <a href="https://example.com">Read the full notes</a>.</p>`,
  ].join('\n')
}

function generate(): Article[] {
  const r = rng(42)
  const now = Date.now()
  const list: Article[] = []
  for (let i = 0; i < ARTICLE_COUNT; i++) {
    const lead = paragraph(r)
    list.push({
      id: `a${i}`,
      feedId: pick(r, feeds).id,
      title: `${pick(r, subjects)} ${pick(r, verbs)}`,
      author: pick(r, authors),
      url: `https://example.com/articles/${i}`,
      published: now - i * (60 * DAY / ARTICLE_COUNT) - Math.floor(r() * 3600_000),
      summary: lead,
      html: body(r, lead),
    })
  }
  return list
}

export const articles: Article[] = generate()

/** Roughly the newest 300 articles start unread, and every 37th is starred. */
export const initialRead: string[] = articles.slice(300).map((a) => a.id)
export const initialStarred: string[] = articles.filter((_, i) => i % 37 === 0).map((a) => a.id)
