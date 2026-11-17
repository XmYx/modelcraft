# Release-note fragments

Each pull request with a user-visible change adds one file here, `<issue>-<branch-slug>.md`,
holding its release-note bullet (one or more `- ` lines, in the user's words). The release flow
(craft-common-actions `scripts/cut_release.py`) joins them into the release notes; pull requests
never edit a release-notes file directly. Maintainers cut releases and tags; contributors never
create release branches or tags.
