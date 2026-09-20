This folder is bundled into the installer as the "cad-engine" resource.

Release builds put two files here before `cargo tauri build` (see scripts/build-engine-pack.py and
.github/workflows/release.yml):

    cad-engine.tar.zst   relocatable CPython + build123d (the CAD engine), one archive
    cad-engine.json      its manifest: engine version, SHA-256, sizes

The app unpacks the archive into its data directory the first time the code-CAD panel is opened
(src-tauri/src/engine_pack.rs). Both files are git-ignored: they are build products, ~200 MB.

Development builds only contain this README; the app then looks for the developer engine installed by
scripts/setup-cad-engine.ps1.
