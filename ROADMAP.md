# ModelCraft roadmap

**Stage: preview** · next: modeling essentials (M1), then UVs and the TextureCraft hand-off (M2)

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** major · **Target:** Cinema 4D (ease), Maya (depth), ZBrush (sculpting); see [docs/target-app-parity.md](docs/target-app-parity.md)

## Headline numbers

| | Value | Kind |
|---|---|---|
| Essentials in the parity map | 59 % of 125 rows | counted |
| Distance to the full products | ≈ 12 % | estimated |
| Viewport, 2.0 M triangles (RTX 4090) | 296 fps orbit, 283 fps sculpting | measured |
| Sculpt dab, 1.0 M quads | 0.2–0.4 ms engine, 1.2 ms in app | measured |
| Tests | 84 unit, property and UI tests | counted |

### Readiness by audience

| Audience | Stage |
|---|---|
| Blocking out models for TextureCraft / ArtCraft | preview |
| Sculpting rough forms | preview |
| Professional modeling, rigging, animation | not yet |

## Features

See [docs/target-app-parity.md](docs/target-app-parity.md) (feature map) and
[docs/gaps.md](docs/gaps.md) (ranked work list). Plan: [docs/roadmap.md](docs/roadmap.md).

## Languages

English source texts; translations through craft-i18n. See
[docs/localization-parity.md](docs/localization-parity.md).

## Agents: CLI and MCP

`modelcraft-cli` runs every engine command (`run`, `exec`, `control` JSON lines); an MCP server is
planned. See [docs/cli.md](docs/cli.md).

## Progress log

| Date | What landed |
|---|---|

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | major | First roadmap with the vertical slice |
