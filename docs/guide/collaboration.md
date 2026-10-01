# Everyday collaboration

A Circle gives everyone shared context, a conversation, and a way to divide
work. Open it with `enox open`, or use the commands below. With several Circles,
add `--circle <name>` to each command.

## Keep the shared folder useful

Use the folder for notes, decisions, working text, and hand-offs. A simple layout:

```text
devices/    where work lives and what each device can run
notes/      shared research and decisions
handoffs/   context for the next person or agent
```

Keep repositories and build output in local checkouts outside the Circle.
Record the device, path, and branch in a note so others know where to continue.
The folder's `AGENTS.md` describes its conventions for agents.

Edit files normally. Changes sync while enoxian is running; edits made offline
sync after you reconnect. Review the latest shared state before a large edit.

## Talk and see who is around

```sh
enox who
enox say "The meeting notes are ready for review."
enox chat -f
```

Chat is shared with Circle members. Mentions can also reach configured agents;
see [using agents](agents.md).

## Divide work with tasks

```sh
enox task-create "Summarize the meeting" --description "Record decisions and next steps"
enox tasks
enox claim <task-id>
```

Claim a task before starting so other participants know you are handling it.
After finishing:

```sh
enox done <task-id>
```

If you will not finish it, return it to the open pool with
`enox unclaim <task-id>`. Claims sync between devices; after reconnecting,
check `enox tasks` again before a large step to confirm you still hold the task.

## Reserve a shared file while editing

For a file several people may edit at once:

```sh
enox bind notes/decisions.md
```

Check `enox status` for existing locks. Wait if someone else holds the file.
Release your lock as soon as you finish:

```sh
enox release notes/decisions.md
```

Locks are advisory: they tell collaborators to wait, but do not change file
permissions. A default lock lasts ten minutes; repeat `bind` to renew it if you
are still working. See the [CLI reference](cli.md#bind) for lease options.

## Review what changed

Open **HISTORY** in the web UI, or inspect proposals from the terminal:

```sh
enox proposal list
enox proposal show <proposal-id>
```

See [reviewing changes](../concepts/proposals.md) before accepting, rejecting,
or reverting a proposal.
