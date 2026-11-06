#!/bin/bash
# Checks the build jobs' artifacts and assembles the release files, for the
# release workflow's publish job and its pull-request dry run (same code).
#
#   collect-release.sh <version> <channel> "<platforms>"
#
# Input: artifacts/<name>/... as download-artifact (no merge-multiple) left
# them. Every expected artifact must be there, holding exactly its expected
# files (regular files, no symlinks, nothing extra), and no other artifact
# may exist. Output: dist/ with those files, SHA256SUMS and the UNSIGNED
# manifest-body.json; `files` lists exactly what to upload.
set -euo pipefail
version=$1 channel=$2 platforms=$3
here="$(cd "$(dirname "$0")" && pwd)"

# Entries: <artifact>=<file>[,<file>...]
expect="ui=mvs-ui-${version}.tar.gz"
for p in $platforms; do
  b="mvs-${version}-$p"
  case "$p" in
    windows-*) expect="$expect desktop-$p=$b.zip,$b-setup.exe" ;;
    linux-*) expect="$expect desktop-$p=$b.tar.gz,$b.AppImage,$b.deb" ;;
    *) echo "::error::no file list for platform $p"; exit 1 ;;
  esac
done

want_dirs=$(for e in $expect; do echo "${e%%=*}"; done | sort)
have_dirs=$(find artifacts -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)
if [ "$have_dirs" != "$want_dirs" ]; then
  echo "::error::unexpected artifact set:"; echo "$have_dirs"; exit 1
fi

rm -rf dist
mkdir dist
: > files
for e in $expect; do
  name="${e%%=*}" list="${e#*=}"
  want=$(echo "$list" | tr ',' '\n' | sort)
  have=$(find "artifacts/$name" -mindepth 1 -printf '%P\n' | sort)
  if [ "$have" != "$want" ]; then
    echo "::error::artifact $name must hold exactly: $list; has:"; ls -lA "artifacts/$name"; exit 1
  fi
  for file in $want; do
    if [ ! -f "artifacts/$name/$file" ] || [ -L "artifacts/$name/$file" ]; then
      echo "::error::$name/$file is not a regular file"; exit 1
    fi
    cp "artifacts/$name/$file" dist/
    echo "$file" >> files
  done
done
ls -l dist

mapfile -t list < files
(cd dist && sha256sum "${list[@]}" > SHA256SUMS && cat SHA256SUMS)

echo SHA256SUMS >> files

# "What's new" for the signed manifests: the version's CHANGELOG section,
# Hebrew before its "---" line, English after (none for dry runs).
notes_args=()
if section=$(node "$here/changelog-section.mjs" "$version" CHANGELOG.md 2>/dev/null); then
  mkdir -p notes
  printf '%s\n' "$section" | awk '/^---$/{exit} {print}' > notes/he.txt
  printf '%s\n' "$section" | awk 'f{print} /^---$/{f=1}' | sed '/./,$!d' > notes/en.txt
  notes_args=(--notes-dir notes)
fi

# One UNSIGNED body per update channel (#211): every release goes to beta;
# one promoted to stable is also signed for alpha. The owner signs offline.
# $channel is kept for compatibility and ignored here.
for ch in beta alpha; do
  node "$here/manifest-body.mjs" --version "$version" --channel "$ch" "${notes_args[@]}" \
    --platforms "$platforms" --dir dist --days 14 --out "dist/manifest-body-$ch.json"
  echo "manifest-body-$ch.json" >> files
done
cat dist/manifest-body-beta.json
