{
  description = "Glyphlow - A keyboard-driven UI navigator for macOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    let
      version = "0.4.1";
      systems = [ "aarch64-darwin" ];
      forEachSystem = flake-utils.lib.eachSystem systems;
    in
    forEachSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # One derivation per release archive. Each archive contains a single
        # binary named after its package, so installPhase can derive the name
        # from pname.
        #
        # NOTE: `.github/workflows/update-nix.yml` rewrites `version` and both
        # `hash` values via sed. It locates each hash by the archive name on the
        # line directly above it, so every `url` must stay immediately above its
        # `hash`. Both archives are cut from the same release tag, hence the one
        # shared `version`.
        mkBinary =
          {
            pname,
            description,
            longDescription,
            src,
            completions ? false,
          }:
          pkgs.stdenv.mkDerivation {
            inherit pname version src;

            nativeBuildInputs = pkgs.lib.optionals completions [ pkgs.installShellFiles ];

            sourceRoot = ".";

            # The completion scripts must be installed from *inside*
            # `installPhase`, not from `postInstall`. Overriding `installPhase`
            # with a plain string replaces stdenv's `installPhase` *function*
            # (`eval "${!curPhase:-$curPhase}"` in stdenv's setup), and only
            # that function runs `runHook postInstall`. A `postInstall` on such
            # a derivation is dead code: the build succeeds and silently ships
            # no completions at all.
            #
            # The scripts are emitted by the freshly installed binary itself, so
            # they can never drift from its argument parser. They land in the
            # standard share/{bash-completion,zsh,fish} locations, which
            # home-manager and nix-darwin wire up for anything on PATH.
            #
            # Guarded on canExecute because the scripts come from running the
            # just-built binary: under cross compilation it cannot run, the
            # substitutions would yield zero-byte files, and
            # installShellCompletion treats that as a build failure.
            #
            # `completions` is only ever set for the client: the server takes no
            # arguments and has no `complete` subcommand. Because the scripts
            # live in the `glyphlow-cli` package itself, they are installed
            # exactly when that package is -- i.e. only when the modules add it
            # under `cli.enable`. Nothing else in the flake installs completions.
            installPhase =
              ''
                install -Dm755 ${pname} $out/bin/${pname}
              ''
              + pkgs.lib.optionalString (
                completions && pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform
              ) ''
                installShellCompletion --cmd ${pname} \
                  --bash <($out/bin/${pname} complete bash) \
                  --zsh <($out/bin/${pname} complete zsh) \
                  --fish <($out/bin/${pname} complete fish)
              '';

            meta = {
              inherit description longDescription;
              homepage = "https://github.com/blindFS/Glyphlow";
              license = pkgs.lib.licenses.mit;
              platforms = [ "aarch64-darwin" ];
            };
          };

        # Server only. Install this if you do not want the command line client.
        glyphlow = mkBinary {
          pname = "glyphlow";
          description = "A keyboard-driven UI navigator for macOS";
          longDescription = ''
            The Glyphlow server: a keyboard-driven UI navigator for macOS.

            Note: You must manually grant Accessibility permissions to the glyphlow binary
            in System Settings > Privacy & Security > Accessibility for it to function.
          '';
          src = pkgs.fetchurl {
            url = "https://github.com/blindFS/Glyphlow/releases/download/v${version}/glyphlow.tar.gz";
            hash = "sha256-gSmf30Va7ZSIc30xOsVNfnOF787mV5OYpS8bli10ga0=";
          };
        };

        # Client only. Useless on its own -- it talks to a running server.
        # The hash below is a placeholder until the first release that ships
        # this archive; update-nix.yml fills it in from the release artifact.
        #
        # `completions = true` is what ships the bash/zsh/fish scripts. They
        # ride along in this package's own `share/`, so they are installed
        # exactly when this package is: the modules only add it under
        # `cli.enable`, and nothing else in the flake installs completions.
        glyphlow-cli = mkBinary {
          pname = "glyphlow-cli";
          description = "Command line client for the Glyphlow server";
          longDescription = ''
            A client that hands activation and workflow requests to a running
            Glyphlow server over a Unix socket.
          '';
          completions = true;
          src = pkgs.fetchurl {
            url = "https://github.com/blindFS/Glyphlow/releases/download/v${version}/glyphlow-cli.tar.gz";
            hash = "sha256-SOoJrQRCPhO/bkzFaBvI6r998Skr9IsqfJts3LT8Cz8=";
          };
        };
      in
      {
        packages.default = glyphlow;
        packages.glyphlow = glyphlow;
        packages.glyphlow-cli = glyphlow-cli;

        # mkApp defaults exePath to /bin/<pname>, which is correct for both.
        apps.default = flake-utils.lib.mkApp { drv = glyphlow; };
        apps.glyphlow = flake-utils.lib.mkApp { drv = glyphlow; };
        apps.glyphlow-cli = flake-utils.lib.mkApp { drv = glyphlow-cli; };
      }
    )
    // {
      # Nix-darwin module
      darwinModules.glyphlow =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.glyphlow;
          tomlFormat = pkgs.formats.toml { };
        in
        {
          options.services.glyphlow = {
            enable = lib.mkEnableOption "Glyphlow service";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow;
              description = "The glyphlow server package to use.";
            };
            cli = {
              enable = lib.mkEnableOption "installing glyphlow-cli and its shell completion scripts";
              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow-cli;
                description = ''
                  The glyphlow-cli package to use.

                  Its `share/` holds the bash/zsh/fish completion scripts, so
                  they are installed only while `cli.enable` is set.
                '';
              };
            };
            settings = lib.mkOption {
              type = tomlFormat.type;
              default = { };
              description = "Configuration written to $XDG_CONFIG_HOME/glyphlow/config.toml.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages =
              [ cfg.package ] ++ lib.optional cfg.cli.enable cfg.cli.package;

            # Since Glyphlow needs to be a LaunchAgent (runs as user, interacts with UI)
            launchd.user.agents.glyphlow = {
              serviceConfig = {
                ProgramArguments = [ "${cfg.package}/bin/glyphlow" ];
                KeepAlive = false;
                RunAtLoad = true;
                ProcessType = "Interactive";
              };
            };

            # Optionally manage config file at $XDG_CONFIG_HOME/glyphlow/config.toml
            # However, nix-darwin doesn't have a great way to manage user-specific config files directly
            # unless we use something like home-manager or environment.etc (which is system-wide).
            # Most users use home-manager for this.
          };
        };

      # Home-manager module
      homeManagerModules.glyphlow =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.glyphlow;
          tomlFormat = pkgs.formats.toml { };
        in
        {
          options.programs.glyphlow = {
            enable = lib.mkEnableOption "Glyphlow";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow;
              description = "The glyphlow server package to use.";
            };
            cli = {
              enable = lib.mkEnableOption "installing glyphlow-cli and its shell completion scripts";
              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow-cli;
                description = ''
                  The glyphlow-cli package to use.

                  Its `share/` holds the bash/zsh/fish completion scripts, so
                  they are installed only while `cli.enable` is set.
                '';
              };
            };
            settings = lib.mkOption {
              type = tomlFormat.type;
              default = { };
              description = "Configuration written to $XDG_CONFIG_HOME/glyphlow/config.toml.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ] ++ lib.optional cfg.cli.enable cfg.cli.package;

            xdg.configFile."glyphlow/config.toml" = lib.mkIf (cfg.settings != { }) {
              source = tomlFormat.generate "glyphlow-config" cfg.settings;
            };

            # On macOS, home-manager can also manage launchd agents if using the macos module
            launchd.agents.glyphlow = lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
              enable = true;
              config = {
                ProgramArguments = [ "${cfg.package}/bin/glyphlow" ];
                KeepAlive = true;
                RunAtLoad = true;
                ProcessType = "Interactive";
              };
            };
          };
        };
    };
}
