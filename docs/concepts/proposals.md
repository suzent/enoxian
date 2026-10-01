# Reviewing changes

Proposals record file changes so you can inspect who changed what and undo a
change when needed. Open **HISTORY** in the web UI, or use the CLI.

## Inspect a change

```sh
enox proposal list
enox proposal show <proposal-id>
```

A proposal includes a diff and any available author or agent information.
Attribution can be inferred or unknown; it does not always identify a particular
person or process with certainty.

## Understand the status

Ordinary edits and explicitly requested agent work normally appear as
**accepted** history. Those edits already happened in the live folder.

Changes made during unaddressed agent activity can be marked **pending** for
review. Pending does not mean the writes were isolated from the live folder.
Inspect the diff and current file before deciding:

```sh
enox proposal accept <proposal-id>
enox proposal reject <proposal-id>
```

These commands are for proposals that need a review decision. Routine accepted
history does not need another acceptance step.

## Undo a change

```sh
enox proposal revert <proposal-id>
```

Revert attempts to undo that proposal while keeping later edits that do not
overlap. Overlapping changes can produce a conflict instead of overwriting
current work. Inspect the reported paths and resolve the conflict before
continuing. Missing history content can also prevent a revert.

See the [proposal commands](../guide/cli.md#proposals) for options.
