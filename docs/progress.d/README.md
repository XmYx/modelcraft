# Progress-log fragments (RFC-0017)

Each pull request adds one file here, `<issue>-<branch-slug>.md`, holding its row of the
`ROADMAP.md` progress log exactly as it reads there: `| YYYY-MM-DD | what landed |`. Pull requests
never edit `## Progress log` or the status line; a maintainer joins the fragments.
`cargo xtask progress` prints the joined log; `cargo xtask ci` checks the fragments.
