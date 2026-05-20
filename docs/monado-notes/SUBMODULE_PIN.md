# Converting `openxr/` to a git submodule

Today `openxr/` is a vendored snapshot of [Monado](https://gitlab.freedesktop.org/monado/monado) sitting as an untracked directory in the ALVR repo. This document explains how to convert it into a proper git submodule when the maintainer is ready.

**It is not safe to automate this step** because we'd have to know the exact upstream Monado commit the snapshot was taken from, and because the conversion replaces the local `openxr/` directory contents with whatever the submodule points at. Anyone working in the tree could lose in-flight Monado-side patches.

## Architectural conflict to resolve first

The Monado-side ALVR driver and the Monado-side ALVR compositor (added under Phase 2 — see [`openxr-migration.md`](../../openxr-migration.md)) currently live INSIDE `openxr/`:

```
openxr/src/xrt/drivers/alvr/                   ← ALVR-specific code
openxr/src/xrt/compositor/alvr/                ← ALVR-specific code
openxr/src/xrt/targets/common/target_builder_alvr.c   ← ALVR-specific code
```

Converting `openxr/` to a clean upstream submodule will erase all three of these. Pick one of these resolutions before you run the conversion:

| Option | What it means | Trade-off |
| --- | --- | --- |
| **A. Fork branch** (recommended) | Maintain a `alvr-org/monado` fork on GitLab/GitHub, push the ALVR-side patches there. The submodule points at the fork's branch instead of upstream main. | One extra repo. Rebasing onto new upstream releases is normal git work. |
| **B. Patch overlay** | Keep the ALVR-side files in a separate top-level directory (e.g. `alvr/monado_overlay/`). At build time `cargo xtask build-openxr-runtime` copies / symlinks them into `openxr/src/xrt/drivers/alvr/` etc. | The submodule stays pristine. Build flow gets one extra step. Subtle bugs if the overlay drifts from what `target_builder_alvr.c` expects. |
| **C. Upstream contributions** | PR the changes into Monado proper. | Long lead time; not in our control. |

Option A is the standard pattern for "vendored open-source dependency with local patches" and is what I'd recommend.

## Conversion procedure (Option A, fork branch)

Assumes you have already created `https://gitlab.freedesktop.org/<your-org>/monado` and pushed an `alvr` branch carrying the contents of `openxr/` plus the ALVR-side patches.

```sh
# Step 1. Make sure docs/monado-notes/ and openxr-migration.md are committed first —
# they live OUTSIDE openxr/ so they will not be touched, but commit them so they
# are recoverable.
git add docs/monado-notes/ openxr-migration.md
git commit -m "docs(monado): vendor Monado reference notes outside openxr/"

# Step 2. Move openxr/ aside and replace it with the submodule.
git rm -rf --cached openxr
mv openxr openxr.snapshot.bak
git submodule add -b alvr https://gitlab.freedesktop.org/<your-org>/monado.git openxr
git submodule update --init --recursive openxr

# Step 3. Diff the live submodule contents against the backup to make sure
# nothing we cared about is missing. Anything new in openxr.snapshot.bak that
# isn't in the submodule should be moved to the fork's alvr branch and pushed.
diff -ruN openxr openxr.snapshot.bak | less

# Step 4. Once you're satisfied, delete the backup and the placeholder
# commented-out block in .gitmodules.
rm -rf openxr.snapshot.bak
# (manually uncomment the [submodule "openxr"] block in .gitmodules and
#  delete the surrounding NOTE comment)

# Step 5. Update CLAUDE.md so future sessions know openxr/ is a submodule:
#   - mention `git submodule update --init --recursive openxr` in setup
#   - drop the "snapshot" language from any descriptions
```

## Verifying the pin

After conversion:

```sh
git -C openxr rev-parse HEAD                    # the commit the submodule is at
git config --file .gitmodules submodule.openxr.url
git submodule status                            # should list openxr alongside openvr
```

The pinned commit is part of the parent-repo commit. To bump:

```sh
cd openxr
git fetch origin
git checkout <new-rev-or-tag>
cd ..
git add openxr
git commit -m "chore(openxr): bump Monado submodule to <new-rev-or-tag>"
```

## Why we don't do this now

* We don't yet know exactly which upstream commit the current snapshot in `openxr/` corresponds to. Pinning to "main" today would change the contents under us without us noticing.
* Phase 2 of the OpenXR-mode integration (the Monado-side ALVR driver + compositor) currently lives inside `openxr/`. Resolving the architectural conflict (Option A/B/C above) is a design decision the maintainer should make explicitly.
* `git submodule add` against an existing populated directory is destructive. Doing it from an automated session would risk losing whatever local edits exist.

When you're ready, the steps above are the recipe.
