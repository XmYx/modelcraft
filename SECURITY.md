# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub:
**[Report a vulnerability](https://github.com/storytold/modelcraft/security/advisories/new)**
(Security tab › Advisories › Report a vulnerability).

Don't open a public issue or pull request for a vulnerability until a fix is released.

Useful to include:
- the ModelCraft version or commit, and the platform (macOS, Windows, Linux, BSD);
- what an attacker can do, and what they need (a crafted project or model file, control-channel access, a local account…);
- steps or a sample file that reproduce it.

We'll acknowledge the report, keep you updated while we work on a fix, and credit you in the advisory unless you'd rather not be named.

## Supported versions

Fixes go into `main` and ship in the next release. Older releases don't get backported fixes.

## Scope

In scope: anything that lets a project, model file (glTF, OBJ, STL, PLY), command, control-channel request or crash journal read or write files it shouldn't, run code, leak data, or crash ModelCraft and lose the user's work.

Crashes from hostile input count, because ModelCraft treats every panic as a bug (see [Never crash](AGENTS.md)).
