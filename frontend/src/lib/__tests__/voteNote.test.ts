import { describe, it, expect } from 'vitest'
import { splitVoteNote } from '../voteNote'

describe('splitVoteNote', () => {
  it('takes the daemon verdict off the end of a posted answer', () => {
    const text = '理由在这里。\n\n答案：上海\n\n_Blind vote: 2 of 2 agreed._'
    expect(splitVoteNote(text)).toEqual({
      body: '理由在这里。\n\n答案：上海',
      note: '2 of 2 agreed.',
    })
  })

  it('leaves an ordinary reply untouched', () => {
    const text = '没有投票，只是一句回复。'
    expect(splitVoteNote(text)).toEqual({ body: text })
  })

  it('ignores the phrase when the agent quotes it mid-reply', () => {
    const text = 'The line _Blind vote: 2 of 2 agreed._ appears in the logs.\n\nANSWER: the note'
    expect(splitVoteNote(text)).toEqual({ body: text })
  })

  it('keeps a single newline before the note out of the body', () => {
    expect(splitVoteNote('答案：是\n\n_Blind vote: the only answer (1 passed)._')).toEqual({
      body: '答案：是',
      note: 'the only answer (1 passed).',
    })
  })
})
