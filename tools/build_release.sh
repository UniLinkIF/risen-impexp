#!/bin/sh
# Builds dist/risen_impexp-<version>.zip: the add-on with risen-core.exe inside (bin/),
# packed by Blender's own extension builder (validates the manifest).
#   BLENDER=/path/to/blender.exe tools/build_release.sh
set -e
cd "$(dirname "$0")/.."
BLENDER="${BLENDER:-blender}"
( cd core && cargo build --release )
rm -rf dist/stage && mkdir -p dist/stage/bin
cp addon/risen_impexp/*.py addon/risen_impexp/blender_manifest.toml dist/stage/
cp core/target/release/risen-core.exe dist/stage/bin/
cp LICENSE NOTICE dist/stage/
"$BLENDER" --background --factory-startup --command extension build --source-dir dist/stage --output-dir dist
rm -rf dist/stage
ls -l dist/*.zip
