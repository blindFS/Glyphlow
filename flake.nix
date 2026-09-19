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
      version = "0.3.2";
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

            installPhase = ''
              install -Dm755 ${pname} $out/bin/${pname}
            '';

            # The scripts are emitted by the freshly installed binary itself, so
            # they can never drift from its argument parser. They land in the
            # standard share/{bash-completion,zsh,fish} locations, which
            # home-manager and nix-darwin wire up for anything on PATH.
            #
            # Guarded on canExecute because the scripts come from running the
            # just-built binary: under cross compilation it cannot run, the
            # substitutions would yield zero-byte files, and
            # installShellCompletion treats that as a build failure.
            postInstall = pkgs.lib.optionalString (
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
            hash = "sha256-Q0/BN5JNz9j+TcnT+da9jr26hD1eOqYjInKOuRB00lg=";
          };
        };

        # Client only. Useless on its own -- it talks to a running server.
        # The hash below is a placeholder until the first release that ships
        # this archive; update-nix.yml fills it in from the release artifact.
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
            hash = pkgs.lib.fakeHash;
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
              enable = lib.mkEnableOption "installing glyphlow-cli alongside the Glyphlow server";
              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow-cli;
                description = "The glyphlow-cli package to use.";
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
              enable = lib.mkEnableOption "installing glyphlow-cli alongside the Glyphlow server";
              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.glyphlow-cli;
                description = "The glyphlow-cli package to use.";
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
