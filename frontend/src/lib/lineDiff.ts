// Line-level diff for the proposal review view.
//
// LCS on lines, with common prefix/suffix trimmed first so the quadratic
// part only runs on the changed middle. Falls back to "all removed + all
// added" when the middle is too large to diff interactively.

export interface DiffRow {
  type: 'context' | 'add' | 'del' | 'skip'
  /** 1-based line number in the before text (null for adds/skips). */
  oldLine: number | null
  /** 1-based line number in the after text (null for dels/skips). */
  newLine: number | null
  text: string
}

const MAX_LCS_LINES = 1500

function splitLines(s: string | null): string[] {
  if (!s) return []
  const lines = s.split('\n')
  if (lines[lines.length - 1] === '') lines.pop()
  return lines
}

export function lineDiff(before: string | null, after: string | null): DiffRow[] {
  const a = splitLines(before)
  const b = splitLines(after)
  const rows: DiffRow[] = []

  // Trim common prefix.
  let start = 0
  while (start < a.length && start < b.length && a[start] === b[start]) start++
  // Trim common suffix (not overlapping the prefix).
  let endA = a.length
  let endB = b.length
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) { endA--; endB-- }

  for (let i = 0; i < start; i++) {
    rows.push({ type: 'context', oldLine: i + 1, newLine: i + 1, text: a[i] })
  }

  const midA = a.slice(start, endA)
  const midB = b.slice(start, endB)

  if (midA.length > MAX_LCS_LINES || midB.length > MAX_LCS_LINES) {
    // Too large for interactive LCS — plain removed-then-added blocks.
    midA.forEach((text, i) =>
      rows.push({ type: 'del', oldLine: start + i + 1, newLine: null, text }))
    midB.forEach((text, i) =>
      rows.push({ type: 'add', oldLine: null, newLine: start + i + 1, text }))
  } else if (midA.length > 0 || midB.length > 0) {
    // LCS table over the middle.
    const n = midA.length
    const m = midB.length
    const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0))
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lcs[i][j] = midA[i] === midB[j]
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1])
      }
    }
    let i = 0, j = 0
    while (i < n && j < m) {
      if (midA[i] === midB[j]) {
        rows.push({ type: 'context', oldLine: start + i + 1, newLine: start + j + 1, text: midA[i] })
        i++; j++
      } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
        rows.push({ type: 'del', oldLine: start + i + 1, newLine: null, text: midA[i] })
        i++
      } else {
        rows.push({ type: 'add', oldLine: null, newLine: start + j + 1, text: midB[j] })
        j++
      }
    }
    while (i < n) { rows.push({ type: 'del', oldLine: start + i + 1, newLine: null, text: midA[i] }); i++ }
    while (j < m) { rows.push({ type: 'add', oldLine: null, newLine: start + j + 1, text: midB[j] }); j++ }
  }

  for (let k = 0; endA + k < a.length; k++) {
    rows.push({ type: 'context', oldLine: endA + k + 1, newLine: endB + k + 1, text: a[endA + k] })
  }

  return rows
}

/** Collapse long context runs to `keep` lines around each change, inserting
 *  skip markers with the number of hidden lines. */
export function collapseContext(rows: DiffRow[], keep = 2): DiffRow[] {
  const visible = new Array<boolean>(rows.length).fill(false)
  rows.forEach((row, idx) => {
    if (row.type === 'add' || row.type === 'del') {
      for (let k = Math.max(0, idx - keep); k <= Math.min(rows.length - 1, idx + keep); k++) {
        visible[k] = true
      }
    }
  })
  const out: DiffRow[] = []
  let hidden = 0
  rows.forEach((row, idx) => {
    if (visible[idx]) {
      if (hidden > 0) {
        out.push({ type: 'skip', oldLine: null, newLine: null, text: `${hidden}` })
        hidden = 0
      }
      out.push(row)
    } else {
      hidden++
    }
  })
  if (hidden > 0) out.push({ type: 'skip', oldLine: null, newLine: null, text: `${hidden}` })
  return out
}
