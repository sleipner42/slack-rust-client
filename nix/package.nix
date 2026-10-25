# The build. Kept next to the source on purpose: a new dependency usually brings
# a new system library with it, and this file is where that gets recorded in the
# same commit as the Cargo.toml change.
#
# `cargoLock.lockFile` reads Cargo.lock directly instead of using a cargoHash,
# so there is no hash to refresh when dependencies change. That works because
# every dependency comes from crates.io; if a `git+` dependency is ever added,
# it needs an entry in `cargoLock.outputHashes`.
{
  lib,
  rustPlatform,
  pkg-config,
  cmake,
  wrapGAppsHook3,
  alsa-lib,
  dbus,
  glib-networking,
  gtk3,
  libsoup_3,
  openssl,
  webkitgtk_4_1,
  libGL,
  vulkan-loader,
  wayland,
  libxkbcommon,
  libx11,
  libxcursor,
  libxrandr,
  libxi,
}:

let
  # dlopen'd at runtime by winit (X11/Wayland) and wgpu (GL/Vulkan). These never
  # appear in the RPATH, so without them on LD_LIBRARY_PATH the binary dies at
  # startup with "libX11.so.6: cannot open shared object file".
  runtimeLibs = [
    libGL
    vulkan-loader
    wayland
    libxkbcommon
    libx11
    libxcursor
    libxrandr
    libxi
  ];
in
rustPlatform.buildRustPackage {
  pname = "better-slack";
  version = "0.1.0";

  # assets/ is compiled in via include_bytes!/include_str! (emoji pack, hunspell
  # dictionaries), so it has to be here. target/ must not be: it is hundreds of
  # megabytes and would be copied into the store on every build.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../assets
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    cmake # aws-lc-sys (rustls' aws-lc-rs backend) compiles C with it
    wrapGAppsHook3 # wires up GIO modules and GSettings schemas for the webview
  ];

  buildInputs = [
    alsa-lib # cpal, for notification sounds
    dbus # libdbus-sys links it; notify-rust desktop alerts
    glib-networking # the GIO TLS module -- without it HTTPS in the webview fails
    gtk3
    libsoup_3
    openssl
    webkitgtk_4_1 # wry's webview
  ];

  # Hand the dlopen'd libraries to the wrapper wrapGAppsHook3 already creates,
  # rather than wrapping the binary a second time. /run/opengl-driver/lib is
  # where NixOS exposes the GL and Vulkan drivers.
  preFixup = ''
    gappsWrapperArgs+=(
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}:/run/opengl-driver/lib"
    )
  '';

  meta = {
    description = "Keyboard-driven Slack client";
    homepage = "https://github.com/sleipner42/slack-rust-client";
    mainProgram = "better-slack";
    platforms = lib.platforms.linux;
  };
}
