#!/usr/bin/env bash
# Convert openxr/ from a vendored Monado 25.1.0 snapshot into a git submodule
# pointing at a fork's `alvr` branch.
#
# This script is NOT run automatically. The maintainer runs it manually after
# creating and populating the fork. See SUBMODULE_PIN.md for context and
# PHASE2_MANIFEST.md for what must live on the fork's `alvr` branch.
#
# PREREQUISITES (do these BEFORE running this script)
#
#   1. Create a Monado fork in your GitLab/GitHub org:
#        https://gitlab.freedesktop.org/<your-org>/monado
#      (Recommended: keep it on freedesktop.org since that's where upstream
#       lives; rebases against upstream are normal git work.)
#
#   2. On the fork, create an `alvr` branch based on the v25.1.0 tag:
#        git clone https://gitlab.freedesktop.org/monado/monado.git
#        cd monado
#        git checkout -b alvr v25.1.0
#        git remote rename origin upstream
#        git remote add origin https://gitlab.freedesktop.org/<your-org>/monado.git
#
#   3. Apply the Phase 2 patch onto the alvr branch:
#        git am < /path/to/alvr-repo/docs/monado-notes/phase2_alvr.patch
#
#   4. Hand-author the upstream-file edits listed in PHASE2_MANIFEST.md
#      ("Upstream files that need editing alongside the patch") and commit
#      them. Push the branch:
#        git push -u origin alvr
#
#   5. Edit FORK_URL below to point at your fork.
#
# WHAT THIS SCRIPT DOES (run from the ALVR repo root)
#
#   * Backs up the current openxr/ snapshot to openxr.snapshot.bak/.
#   * Runs `git submodule add -b alvr $FORK_URL openxr`.
#   * Uncomments the [submodule "openxr"] block in .gitmodules.
#   * Diffs the live submodule against the backup so you can spot anything
#     missing on the fork branch BEFORE deleting the backup.
#   * Leaves the backup in place. Delete it once you're satisfied.

set -euo pipefail

# ---- CONFIGURE ME ---------------------------------------------------------
FORK_URL="https://github.com/<your-org>/monado.git"
FORK_BRANCH="alvr"
# ---------------------------------------------------------------------------

if [[ "$FORK_URL" == *"<your-org>"* ]]; then
  echo "ERROR: Edit FORK_URL near the top of this script before running."
  exit 1
fi

if [[ ! -d openxr ]]; then
  echo "ERROR: no openxr/ in the current directory. Run from the ALVR repo root."
  exit 1
fi

if [[ -n "$(git submodule status openxr 2>/dev/null)" ]]; then
  echo "openxr is already a submodule. Aborting."
  exit 1
fi

# Step 1 — make sure docs and migration plan are committed first.
if ! git diff --quiet -- docs/monado-notes/ docs/openxr-migration.md 2>/dev/null; then
  echo "WARN: docs/monado-notes/ or docs/openxr-migration.md have uncommitted changes."
  echo "Commit them before continuing so they survive the conversion cleanly."
  read -r -p "Continue anyway? [y/N] " resp
  [[ "$resp" =~ ^[Yy]$ ]] || exit 1
fi

# Step 2 — drop any tracked files under openxr/ from the index. This happens
# when Phase 2 work committed files like openxr/src/xrt/drivers/alvr/* inside
# the vendored snapshot. `git submodule add` refuses if any index entry exists
# at the target path. --cached keeps the working tree intact so the next step
# can back it up.
if [[ -n "$(git ls-files -- openxr)" ]] || [[ -n "$(git ls-tree HEAD -- openxr 2>/dev/null)" ]]; then
  echo "[1/5] git rm -rf --cached openxr (removing previously-tracked files from index)"
  git rm -rf --cached openxr >/dev/null
fi

# Step 3 — back up the snapshot.
if [[ -d openxr ]]; then
  echo "[2/5] Backing up openxr/ -> openxr.snapshot.bak/"
  mv openxr openxr.snapshot.bak
fi

# Step 4 — add as submodule.
echo "[3/5] git submodule add -b $FORK_BRANCH $FORK_URL openxr"
git submodule add -b "$FORK_BRANCH" "$FORK_URL" openxr
git submodule update --init --recursive openxr

# Step 5 — diff. Anything in the backup that's not in the submodule needs
# investigation: typically means a Phase 2 / Phase 3 file did not get pushed
# to the fork's alvr branch.
echo "[4/5] Diffing live submodule against the backup. Look for ADDITIONS in"
echo "      the backup (lines starting with 'Only in openxr.snapshot.bak/...')."
echo "      Output goes to openxr-submodule-diff.log"
diff -rq openxr openxr.snapshot.bak > openxr-submodule-diff.log 2>&1 || true
echo "      First 30 lines:"
head -30 openxr-submodule-diff.log || true

# Step 6 — uncomment the [submodule "openxr"] block in .gitmodules so future
# clones get the submodule by default.
echo "[5/5] Edit .gitmodules manually if there's a commented-out [submodule \"openxr\"]"
echo "      placeholder block. Uncomment it and verify it matches the live entry"
echo "      git submodule add just wrote."

echo ""
echo "Conversion complete. To finalise:"
echo "  1. Review openxr-submodule-diff.log."
echo "  2. If clean: rm -rf openxr.snapshot.bak openxr-submodule-diff.log"
echo "  3. git add openxr .gitmodules"
echo "  4. git commit -m 'feat(openxr): convert openxr/ to a Monado submodule on the alvr fork'"
