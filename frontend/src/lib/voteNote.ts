/** The verdict enoxian appends to the answer a blind vote settled on.
 *
 *  The daemon posts the winning voter's reply and then adds an italic line of
 *  its own (`_Blind vote: 2 of 2 agreed._`) — see `src/agent/warden/vote.rs`.
 *  Left inside the message body it reads as the agent describing its own vote,
 *  and it lands under the `ANSWER:` line the reply was asked to end with, so
 *  the message both looks misquoted and argues with its own instructions.
 *  Splitting it out lets the transcript show it as what it is: a footnote from
 *  the daemon on a message somebody else wrote.
 *
 *  Matched only at the end of the text, only as the whole of the last
 *  paragraph, so an agent that happens to quote the phrase mid-reply is left
 *  alone.
 */
const VOTE_NOTE = /\n{2,}_Blind vote: ([^_\n]+)_\s*$/

export function splitVoteNote(text: string): { body: string; note?: string } {
  const match = VOTE_NOTE.exec(text)
  if (!match) return { body: text }
  return { body: text.slice(0, match.index).trimEnd(), note: match[1].trim() }
}
