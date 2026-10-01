# Glossary

| Term | Meaning |
|------|---------|
| Circle | A collaboration group with shared files, chat, tasks, and membership. |
| Workspace | The local folder holding a Circle's shared files. |
| Device | A machine participating in a Circle with its own identity. |
| Linked device | Another device associated with your user identity through `enox link`. |
| Member | A device admitted to the Circle under its membership policy. |
| Invite | A private link used to join a Circle. Treat it as a credential. |
| Daemon | The background enoxian process that keeps your Circles connected. |
| Task claim | A signal that a participant is responsible for a task. |
| File lock | A temporary reservation asking others to wait before editing a file. |
| Proposal | A record of file changes that you can inspect and, where appropriate, undo. |
| Agent | An AI tool configured to run on a member's device. |
| Agent session | An agent's conversation and work context in a Circle. |
| Push / pull | Whether this device automatically runs configured agents when mentioned (`push`) or leaves automatic mention execution off (`pull`). |
| Ambient agent | An agent configured to read unaddressed human messages and decide whether to respond. |
| Relay | A service that forwards encrypted traffic when devices cannot connect directly. |

Start with [Circles and shared files](overview.md) or look up a command in the
[CLI reference](../guide/cli.md). Implementation terminology is covered in the
[developer documentation](../development/README.md).
