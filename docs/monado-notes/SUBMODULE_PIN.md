# Converting `openxr/` to a git submodule

Today `openxr/` is a vendored snapshot of [Monado](https://gitlab.freedesktop.org/monado/monado) sitting as an untracked directory in the ALVR repo. This document explains how to convert it into a proper git submodule when the maintainer is ready.

**It is not safe to automate this step** because the conversion replaces the local `openxr/` directory contents with whatever the submodule points at, and anyone working in the tree could lose in-flight Monado-side patches.

**Upstream commit identified (2026-05-20):** the snapshot is **Monado 25.1.0** (tag `v25.1.0`, released 2025-12-09). See [`PHASE2_MANIFEST.md`](PHASE2_MANIFEST.md) for the per-file evidence and the precise list of ALVR-side files that must live on the fork's `alvr` branch.

**Ready-made artefacts in this directory:**
- `phase2_alvr.patch` — `git am`-applicable mailbox patch with the 10 additive Phase 2 files. Apply onto a checkout of `v25.1.0` to populate the fork's `alvr` branch.
- `convert_to_submodule.sh` — annotated conversion script. Edit `FORK_URL` before running. Backs up the current snapshot, runs `git submodule add`, and produces a diff log so any missing-on-fork files are visible before the backup is deleted.
- `PHASE2_MANIFEST.md` — lists upstream-file edits that need to be authored on the fork branch alongside the additive patch (none of those edits exist in the current snapshot yet).

## Architectural conflict to resolve first

The Monado-side ALVR driver and the Monado-side ALVR compositor (added under Phase 2 — see [`openxr-migration.md`](../openxr-migration.md)) currently live INSIDE `openxr/`:

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

The `convert_to_submodule.sh` script in this directory automates the local steps. Before running it, prepare the fork:

**On the fork side (outside this repo):**

```sh
# 1. Create a fork at https://gitlab.freedesktop.org/<your-org>/monado.
# 2. Build the alvr branch:
git clone https://gitlab.freedesktop.org/monado/monado.git
cd monado
git checkout -b alvr v25.1.0
git remote rename origin upstream
git remote add origin https://gitlab.freedesktop.org/<your-org>/monado.git

# 3. Apply the Phase 2 additive patch (10 new files):
git am < /path/to/alvr/docs/monado-notes/phase2_alvr.patch

# 4. Hand-author the upstream-file edits from PHASE2_MANIFEST.md:
#    - openxr/src/xrt/drivers/CMakeLists.txt: add_subdirectory(alvr)
#    - openxr/src/xrt/compositor/CMakeLists.txt: add_subdirectory(alvr)
#    - openxr/src/xrt/targets/common/target_lists.c: t_builder_alvr_create entry
#    - openxr/CMakeLists.txt: option(XRT_BUILD_DRIVER_ALVR) + XRT_FEATURE_COMP_ALVR
#    Commit each edit.

# 5. Push:
git push -u origin alvr
```

**On the ALVR side (inside this repo):**

```sh
# Edit FORK_URL near the top of convert_to_submodule.sh, then:
bash docs/monado-notes/convert_to_submodule.sh
```

The script: backs up `openxr/` to `openxr.snapshot.bak/`, runs `git submodule add -b alvr <fork> openxr`, and produces `openxr-submodule-diff.log` so any missing-on-fork files are visible before the backup is deleted.

**After the script succeeds:**

```sh
# Inspect openxr-submodule-diff.log. If clean:
rm -rf openxr.snapshot.bak openxr-submodule-diff.log

# Uncomment the [submodule "openxr"] block in .gitmodules.

# Update CLAUDE.md:
#   - mention `git submodule update --init --recursive openxr` in setup
#   - drop "snapshot" language from descriptions
git add openxr .gitmodules CLAUDE.md
git commit -m "feat(openxr): convert openxr/ to a Monado submodule on the alvr fork"
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

## Why we don't do this from an automated session

* `git submodule add` against an existing populated directory is destructive — it overwrites whatever's there with the submodule's contents. From an unattended session we couldn't tell whether the openxr/ directory has in-flight maintainer edits.
* The fork needs to exist and the `alvr` branch needs to be populated and pushed before the local conversion is useful. Those steps require credentials we don't hold.

The upstream-pin uncertainty that motivated the earlier "don't do this now" framing is **resolved**: the snapshot is Monado 25.1.0 (see top of this file and `PHASE2_MANIFEST.md`). When the fork is ready and `FORK_URL` is set in the script, the conversion is just `bash convert_to_submodule.sh`.
