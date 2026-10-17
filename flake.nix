{
  description = "Clarion: A Wayland compositor with an independent desktop core and native Vulkan backend.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      revision = self.shortRev or self.dirtyShortRev or "unknown";
      clarion-package =
        {
          lib,
          cairo,
          dbus,
          fontconfig,
          freetype,
          libGL,
          libdisplay-info_0_3,
          libinput,
          seatd,
          libxkbcommon,
          libgbm,
          pango,
          python3,
          pipewire,
          pkg-config,
          rustPlatform,
          systemd,
          wayland,
          vulkan-headers,
          vulkan-loader,
          xorg,
          installShellFiles,
          withDbus ? true,
          withSystemd ? true,
          withScreencastSupport ? true,
          withDinit ? false,
        }:

        rustPlatform.buildRustPackage {
          pname = "clarion";
          version = revision;

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./build.rs
              ./clarion-config
              ./clarion-core
              ./clarion-motion
              ./clarion-timing
              ./clarion-scene
              ./clarion-ipc
              ./clarion-settings
              ./clarion-launcher
              ./clarion-shell/assets/shell/ui-text.ttf
              ./clarion-shell/assets/shell/cjk-regular.otf
              ./clarion-visual-tests
              ./resources
              ./src
              ./third_party/smithay
              ./third_party/gpui
              ./Cargo.toml
              ./Cargo.lock
            ];
          };

          postPatch = ''
            patchShebangs resources/clarion-session
            substituteInPlace resources/clarion.desktop \
              --replace-fail 'Exec=clarion-session' "Exec=$out/bin/clarion-session"
            substituteInPlace resources/clarion.service \
              --replace-fail 'ExecStart=clarion' "ExecStart=$out/bin/clarion"
          '';

          cargoLock = {
            # NOTE: This is only used for Git dependencies
            allowBuiltinFetchGit = true;
            lockFile = ./Cargo.lock;
          };

          strictDeps = true;

          nativeBuildInputs = [
            rustPlatform.bindgenHook
            pkg-config
            installShellFiles
          ];

          buildInputs =
            [
              cairo
              dbus
              fontconfig
              freetype
              libGL
              libdisplay-info_0_3
              libinput
              seatd
              libxkbcommon
              libgbm
              pango
              wayland
              vulkan-headers
              vulkan-loader
              # GPUI's headless test-support also compiles the X11 backend.
              # xkbcommon-x11 is provided by libxkbcommon above.
              xorg.libxcb
            ]
            ++ lib.optional (withDbus || withScreencastSupport || withSystemd) dbus
            ++ lib.optional withScreencastSupport pipewire
            # Also includes libudev
            ++ lib.optional withSystemd systemd;

          buildFeatures =
            lib.optional withDbus "dbus"
            ++ lib.optional withDinit "dinit"
            ++ lib.optional withScreencastSupport "xdp-gnome-screencast"
            ++ lib.optional withSystemd "systemd";
          buildNoDefaultFeatures = true;

          # clarion runs an actual instance of the compositor (with a mock backend) during
          # tests and thus creates a real socket file in the runtime dir.
          # this is fine for our build, we just need to make sure it has a directory to write to.
          preCheck = ''
            export XDG_RUNTIME_DIR="$(mktemp -d)"
          '';

          checkFlags = [
            # These tests require the ability to access a "valid EGL Display", but that won't work
            # inside the Nix sandbox
            "--skip=::egl"
          ];

          cargoBuildFlags = [
            "--locked"
            "--workspace"
            "--exclude"
            "clarion-visual-tests"
          ];
          cargoTestFlags = [
            "--locked"
            "--workspace"
            "--exclude"
            "clarion-visual-tests"
          ];

          postInstall =
            ''
              installShellCompletion --cmd clarion \
                --bash <($out/bin/clarion completions bash) \
                --fish <($out/bin/clarion completions fish) \
                --nushell <($out/bin/clarion completions nushell) \
                --zsh <($out/bin/clarion completions zsh)

              install -Dm755 target/''${cargoBuildType:-release}/clarion-settings $out/bin/clarion-settings
              install -Dm755 target/''${cargoBuildType:-release}/clarion-launcher $out/bin/clarion-launcher
              install -Dm755 resources/clarion-session $out/bin/clarion-session
              install -Dm755 resources/clarion-shell-msg $out/bin/clarion-shell-msg
              substituteInPlace $out/bin/clarion-shell-msg \
                --replace-fail '#!/usr/bin/env python3' '#!${python3}/bin/python3'
              for legacy in realand realand-session realand-settings realand-shell-msg; do
                install -Dm755 resources/compat/$legacy $out/bin/$legacy
              done
              install -Dm644 resources/clarion.desktop -t $out/share/wayland-sessions
              install -Dm644 resources/clarion-portals.conf -t $out/share/xdg-desktop-portal
            ''
            + lib.optionalString withSystemd ''
              install -Dm644 resources/clarion{.service,-shutdown.target} -t $out/lib/systemd/user
            ''
            + lib.optionalString withDinit ''
              install -Dm644 resources/dinit/clarion $out/lib/dinit.d/user/clarion
              install -Dm644 resources/dinit/clarion.target $out/lib/dinit.d/user/clarion.target
            '';

          env = {
            # Keep compositor and GPUI libraries in the runtime closure/RPATH,
            # including libraries discovered through `dlopen()` by either client.
            RUSTFLAGS = toString (
              map (arg: "-C link-arg=" + arg) [
                "-Wl,--push-state,--no-as-needed"
                "-lEGL"
                "-lwayland-client"
                "-lxkbcommon"
                "-lfontconfig"
                "-lfreetype"
                "-lvulkan"
                "-Wl,--pop-state"
              ]
            );
            CLARION_BUILD_COMMIT = revision;
          };

          passthru = {
            providedSessions = [ "clarion" ];
          };

          meta = {
            description = "Wayland compositor with an independent desktop core and native Vulkan backend";
            homepage = "https://github.com/123llyrr/clarion";
            license = lib.licenses.gpl3Plus;
            mainProgram = "clarion";
            platforms = lib.platforms.linux;
          };
        };

      inherit (nixpkgs) lib;
      # Support all Linux systems that the nixpkgs flake exposes
      systems = lib.intersectLists lib.systems.flakeExposed lib.platforms.linux;

      forAllSystems = lib.genAttrs systems;
      nixpkgsFor = forAllSystems (system: nixpkgs.legacyPackages.${system});
    in
    {
      checks = forAllSystems (system: {
        # We use the debug build here to save a bit of time
        inherit (self.packages.${system}) clarion-debug;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
          rustfmt' = pkgs.rustfmt.override { asNightly = true; };
          inherit (self.packages.${system}) clarion;
        in
        {
          default = pkgs.mkShell {
            packages = builtins.attrValues {
              inherit (pkgs)
                rustc
                cargo
                clippy
                cargo-insta
                ;
              inherit rustfmt';
            };

            nativeBuildInputs = [
              pkgs.rustPlatform.bindgenHook
              pkgs.pkg-config
              pkgs.wrapGAppsHook4 # For `clarion-visual-tests`
            ];

            buildInputs = clarion.buildInputs ++ [
              pkgs.libadwaita # For `clarion-visual-tests`
            ];

            env = {
              # WARN: Do not overwrite this variable in your shell!
              # It is required for `dlopen()` to work on some libraries; see the comment
              # in the package expression
              #
              # This should only be set with `RUSTFLAGS="$RUSTFLAGS -C your-flags"`
              RUSTFLAGS = clarion.RUSTFLAGS;
            };
          };
        }
      );

      formatter = forAllSystems (system: nixpkgsFor.${system}.nixfmt-rfc-style);

      packages = forAllSystems (
        system:
        let
          clarion = nixpkgsFor.${system}.callPackage clarion-package { };
        in
        {
          inherit clarion;

          # NOTE: This is for development purposes only
          #
          # It is primarily to help with quickly iterating on
          # changes made to the above expression - though it is
          # also not stripped in order to better debug clarion itself
          clarion-debug = clarion.overrideAttrs (
            newAttrs: oldAttrs: {
              pname = oldAttrs.pname + "-debug";

              cargoBuildType = "debug";
              cargoCheckType = newAttrs.cargoBuildType;

              dontStrip = true;
            }
          );

          default = clarion;
        }
      );

      overlays.default = final: _: {
        clarion = final.callPackage clarion-package { };
      };
    };
}
