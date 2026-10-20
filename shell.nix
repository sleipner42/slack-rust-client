# Linux development shell: `nix-shell`, then `cargo run --release --locked`.
{ pkgs ? import <nixpkgs> { } }:

let
  runtimeLibraries = with pkgs; [
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    clippy
    cmake
    pkg-config
    rustc
    rustfmt
  ];

  buildInputs = with pkgs; [
    alsa-lib
    dbus
    glib-networking
    gtk3
    libsoup_3
    openssl
    webkitgtk_4_1
  ] ++ runtimeLibraries;

  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
  # WebKitGTK loads TLS support from this GIO module at runtime.
  GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules";
}
