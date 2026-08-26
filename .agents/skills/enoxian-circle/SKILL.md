---
name: enoxian-circle
description: Manage enoxian Circle tasks, chat, members, and file locks.
---

# Enoxian Circle Collaboration

Use enoxian to coordinate people and agents in a shared Circle. Its CLI is the control plane for presence, conversation, tasks, membership, and file locks. When work involves workspace files, continue to read, edit, search, and test them with the host agent's native tools; never treat `enox` as a file proxy.

## Establish the Circle

1. Read the repository's applicable agent instructions. They may impose stricter coordination rules.
2. Run `enox circles` to verify that the daemon is reachable and see the available Circles.
3. If the daemon is unavailable and starting the local service is within the requested workflow, run `enox start`, then retry.
4. Select the Circle from an explicit user choice, `ENOXIAN_CIRCLE`, repository instructions, or other unambiguous context. If several Circles remain plausible, do not guess; ask the user.
5. Pass `--circle <name-or-id-prefix>` to commands whenever selection would otherwise be ambiguous. Use `--json` when structured output materially helps.

## Identify Agents

Enoxian applies the same device-vouched actor model to managed and independent
agents, but transports the identity differently.

### Enoxian-managed agents

Agents launched by `enox agent run` or a pushed mention are registered by the
daemon automatically. Do not ask them to register again or put a token in their
prompt.

- Enoxian binds native write/edit operations to the managed change session, so
  file tools do not need to carry a token.
- The managed process tree inherits `ENOXIAN_ACTOR_TOKEN`,
  `ENOXIAN_AGENT_ID`, and `ENOXIAN_CIRCLE`. The `enox` CLI consumes the token
  automatically when the agent uses a shell tool for chat, tasks, or locks.
- Never print, copy, or post the inherited token. It is process plumbing, not
  agent context.

### Independently spawned agents

When an agent was launched outside Enoxian's managed agent process and its shell
or environment may not persist, register it once and pass the returned token on
every mutating CLI call:

```text
enox register <agent-label> --circle <circle>
enox claim <task-id> --circle <circle> --token <token>
enox say "<message>" --circle <circle> --token <token>
enox bind <path> --circle <circle> --token <token>
```

- Treat the token as a one-hour bearer secret. Do not post it to Circle chat,
  commit it, or include it in user-visible logs.
- The token is bound to the selected Circle and the issuing device's
  cryptographic peer ID. Another device cannot replay it.
- The label is device-vouched, not process-authenticated. Agents on the same
  device can use one another's tokens; do not claim stronger identity guarantees.
- Registration is lost when the daemon restarts. Register again if a token is
  expired or rejected.
- `--token` is global and may be placed at the end of a command, which is useful
  for agents whose terminal or environment does not persist between tool calls.
- Actor tokens attribute chat, task creation/claim/completion, and file locks.
  Managed sessions additionally attribute native file writes. Neither mechanism
  grants Circle membership or administrative authority.

## Choose the Coordination Surface

Use only the surfaces needed for the request. Read-only inspection does not authorize a mutation.

### Presence and status

```text
enox who --circle <circle>
enox status --circle <circle>
enox member list --circle <circle>
```

- Use `who` for live or recent agent/device presence.
- Use `member list` for durable membership and roles.
- Use `status` for the broader Circle state, including locks and connectivity.
- Do not infer durable membership, availability, or identity from presence alone.

### Chat

```text
enox chat --circle <circle>
enox chat --follow --circle <circle>
enox say "<message>" --circle <circle>
```

- Read recent chat when the request depends on current group context or prior decisions.
- Post only when the user asks to communicate or when a message is an expected part of an authorized coordination workflow.
- Resolve exact agent IDs from Circle state before using `@agent_id`. A mention may launch an allowlisted agent on another device, so do not mention agents speculatively.
- Keep secrets, private chain-of-thought, and raw file contents out of chat.
- Use `--follow` only for an active monitoring request, and stop it when the monitoring objective is met.

### Tasks

```text
enox tasks --circle <circle>
enox task-create "<title>" --description "<text>" --circle <circle>
enox claim <task-id> --circle <circle>
enox unclaim <task-id> --circle <circle>
enox done <task-id> --circle <circle>
```

- Create a task when the user requests shared tracking or applicable repository instructions require one. Use a concrete outcome as the title and put scope or acceptance details in the description.
- Claim an existing matching task before starting its work. Claim only one task at a time and never claim a merely similar or unrelated task.
- If you stop owning a claimed task without completing it, return it to the
  open pool with `unclaim`; only the recorded claimant on the claiming device
  can do this. Do not unclaim another collaborator's work.
- A direct request absent from the task list does not justify claiming unrelated work. If repository instructions require a registered task, stop before acting and ask for or create one only with appropriate authority.
- Mark a task done only when its outcome is genuinely complete. Leave incomplete or blocked work unfinished and report the blocker.

### Members and invitations

```text
enox member pending --circle <circle>
enox member approve <peer-id> --circle <circle>
enox member reject <peer-id> --circle <circle>
enox member add <peer-id> --circle <circle>
enox member promote <peer-id> --circle <circle>
enox member remove <peer-id> --circle <circle>
enox invite <circle>
```

- Inspect membership or pending requests freely when relevant.
- Adding, approving, rejecting, promoting, or removing members changes the Circle's trust boundary. Require explicit user authority, verify the exact peer and intended role, and report the result.
- Generate an invite only when requested. Treat the invite URI as a secret-bearing credential: return it only through the intended private channel and never commit it or post it to public chat.
- Do not assume admin authority. If an operation requires an unavailable admin key or role, report that limitation rather than attempting a workaround.

### File coordination

When the authorized work includes shared workspace files, inspect tasks and presence before editing. Acquire an explicit lock for shared, conflict-prone files such as configuration, schema, or central entry points:

```text
enox bind <path> --circle <circle>
```

- Locks are normally optional for a newly created or routine file unlikely to be shared.
- If another agent owns the lock, do not bypass it or change permissions. Wait or coordinate with the owner.
- Preserve unrelated local and remote changes. A task claim or file lock does not grant ownership of the whole worktree.
- Release every explicit lock immediately after the protected work is complete:

  ```text
  enox release <path> --circle <circle>
  ```

## Guardrails

- Do not invent Circle names, task IDs, peer IDs, agent identities, roles, or lock ownership.
- Do not claim multiple tasks to reserve future work or retain locks while idle.
- Do not post messages, create tasks, invite people, or change membership merely because those capabilities are available.
- Joining or leaving a Circle, changing reaction policy, or performing another mutation outside the requested workflow requires explicit user authority.
