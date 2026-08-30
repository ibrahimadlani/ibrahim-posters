# 6. Trunk-based branching over git-flow

Date: 2026-08-30

## Status

Accepted

## Context

The repository was initially set up with `main` and `develop`. That is
git-flow, and git-flow solves a specific problem: coordinating a release train
across several teams when releases are infrequent, versioned artefacts shipped
to users who install them.

This service does not have that problem. It is deployed as a container image,
releases are continuous, and there is currently one contributor. In that
setting a long-lived `develop` branch adds a merge step and a second place for
work to sit without adding any coordination value, and the divergence between
the two branches is pure carrying cost.

The milestone structure the project is planned around — a branch per
milestone, squash-merged, tagged when it lands — is already trunk-based in
shape. Keeping `develop` would mean a promotion step that no milestone needs.

## Decision

Trunk-based development on `main`. `develop` is deleted.

Work happens on short-lived branches prefixed `feat/`, `fix/`, `chore/`,
`docs/`, `refactor/`, `test/` or `perf/`, and is squash-merged into `main`.
Merge commits and rebase merges are disabled at the repository level so that
history is linear by construction rather than by convention.

`main` is protected: linear history required, status checks required
(`fmt`, `clippy`, `test`, `deny`), force pushes and deletion blocked.

Required approvals are set to **zero**, deliberately. A solo repository cannot
satisfy a one-approval rule — the author cannot approve their own pull request
— so requiring one would either block every merge or be routinely bypassed
with admin rights, and a rule that is always bypassed is worse than no rule
because it makes the protection settings misleading. Status checks are the
real gate. When a second contributor joins, the count moves to one.

## Consequences

**Positive.** One integration point. Linear history, so `git bisect` and
`git log` read cleanly and `git-cliff` can generate a changelog from commit
subjects without untangling merge topology. No promotion step between
milestones.

**Negative.** No branch holds "the next release" separately from `main`, so
`main` must always be releasable. That is the intended discipline, but it puts
the full weight of correctness on CI. Squash-merging discards intermediate
commits from a branch, so the pull request becomes the record of how a change
developed.

**Neutral.** Release branches remain available if a hotfix is ever needed
against an old tag. Nothing in this decision prevents cutting one.
