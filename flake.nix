{
  description = "Realand: A Wayland compositor with an independent desktop core and native Vulkan backend.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      revision = self.shortRev or self.dirtyShortRev or "unknown";
      realand-package =
        {
          lib,
          cairo,
          dbus,
          libGL,
          libdisplay-info_0_3,
          libinput,
          seatd,
          libxkbcommon,
          libgbm,
          pango,
          pipewire,
          pkg-config,
          rustPlatform,
          systemd,
          wayland,
          installShellFiles,
          withDbus ? true,
          withSystemd ? true,
          withScreencastSupport ? true,
          withDinit ? false,
        }:

        rustPlatform.buildRustPackage {
          pname = "realand";
          version = revision;

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./build.rs
              ./realand-config
              ./realand-core
              ./realand-ipc
              ./realand-settings
              ./realand-shell/assets/macos/SFNS.ttf
              ./realand-shell/assets/macos/PingFangSC-Regular.otf
              ./realand-visual-tests
              ./resources
              ./src
              ./third_party/smithay
              ./Cargo.toml
              ./Cargo.lock
            ];
          };

          postPatch = ''
            patchShebangs resources/realand-session
            substituteInPlace resources/realand.desktop \
              --replace-fail 'Exec=realand-session' "Exec=$out/bin/realand-session"
            substituteInPlace resources/realand.service \
              --replace-fail 'ExecStart=realand' "ExecStart=$out/bin/realand"
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
              libGL
              libdisplay-info_0_3
              libinput
              seatd
              libxkbcommon
              libgbm
              pango
              wayland
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

          # realand runs an actual instance of the compositor (with a mock backend) during
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
            "--workspace"
            "--exclude"
            "realand-visual-tests"
          ];
          cargoTestFlags = [
            "--workspace"
            "--exclude"
            "realand-visual-tests"
          ];

          postInstall =
            ''
              installShellCompletion --cmd realand \
                --bash <($out/bin/realand completions bash) \
                --fish <($out/bin/realand completions fish) \
                --nushell <($out/bin/realand completions nushell) \
                --zsh <($out/bin/realand completions zsh)

              install -Dm755 target/release/realand-settings $out/bin/realand-settings
              install -Dm755 resources/realand-session $out/bin/realand-session
              install -Dm644 resources/realand.desktop -t $out/share/wayland-sessions
              install -Dm644 resources/realand-portals.conf -t $out/share/xdg-desktop-portal
            ''
            + lib.optionalString withSystemd ''
              install -Dm644 resources/realand{.service,-shutdown.target} -t $out/lib/systemd/user
            ''
            + lib.optionalString withDinit ''
              install -Dm644 resources/dinit/realand $out/lib/dinit.d/user/realand
              install -Dm644 resources/dinit/realand.target $out/lib/dinit.d/user/realand.target
            '';

          env = {
            # Force linking with libEGL and libwayland-client so they end up in RPATH and
            # can be discovered by `dlopen()`
            RUSTFLAGS = toString (
              map (arg: "-C link-arg=" + arg) [
                "-Wl,--push-state,--no-as-needed"
                "-lEGL"
                "-lwayland-client"
                "-Wl,--pop-state"
              ]
            );
            REALAND_BUILD_COMMIT = revision;
          };

          passthru = {
            providedSessions = [ "realand" ];
          };

          meta = {
            description = "Wayland compositor with an independent desktop core and native Vulkan backend";
            homepage = "https://github.com/123llyrr/realand";
            license = lib.licenses.gpl3Plus;
            mainProgram = "realand";
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
        inherit (self.packages.${system}) realand-debug;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
          rustfmt' = pkgs.rustfmt.override { asNightly = true; };
          inherit (self.packages.${system}) realand;
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
              pkgs.wrapGAppsHook4 # For `realand-visual-tests`
            ];

            buildInputs = realand.buildInputs ++ [
              pkgs.libadwaita # For `realand-visual-tests`
            ];

            env = {
              # WARN: Do not overwrite this variable in your shell!
              # It is required for `dlopen()` to work on some libraries; see the comment
              # in the package expression
              #
              # This should only be set with `RUSTFLAGS="$RUSTFLAGS -C your-flags"`
              RUSTFLAGS = realand.RUSTFLAGS;
            };
          };
        }
      );

      formatter = forAllSystems (system: nixpkgsFor.${system}.nixfmt-rfc-style);

      packages = forAllSystems (
        system:
        let
          realand = nixpkgsFor.${system}.callPackage realand-package { };
        in
        {
          inherit realand;

          # NOTE: This is for development purposes only
          #
          # It is primarily to help with quickly iterating on
          # changes made to the above expression - though it is
          # also not stripped in order to better debug realand itself
          realand-debug = realand.overrideAttrs (
            newAttrs: oldAttrs: {
              pname = oldAttrs.pname + "-debug";

              cargoBuildType = "debug";
              cargoCheckType = newAttrs.cargoBuildType;

              dontStrip = true;
            }
          );

          default = realand;
        }
      );

      overlays.default = final: _: {
        realand = final.callPackage realand-package { };
      };
    };
}
