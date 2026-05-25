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
      version = "0.1.1";
      systems = [ "aarch64-darwin" ];
      forEachSystem = flake-utils.lib.eachSystem systems;
    in
    forEachSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # Prebuilt binary derivation
        glyphlow-bin = pkgs.stdenv.mkDerivation {
          pname = "glyphlow";
          inherit version;

          src = pkgs.fetchurl {
            url = "https://github.com/blindFS/Glyphlow/releases/download/v${version}/glyphlow.tar.gz";
            hash = "sha256-jB5FeUJ5U8TlxpI0Lv4q8ckzApnWy5vLDgYTjy2hBRU=";
          };

          nativeBuildInputs = [ pkgs.installShellFiles ];

          sourceRoot = ".";

          installPhase = ''
            install -Dm755 glyphlow $out/bin/glyphlow
          '';

          meta = with pkgs.lib; {
            description = "A keyboard-driven UI navigator for macOS";
            longDescription = ''
              Glyphlow is a keyboard-driven UI navigator for macOS.
              Note: You must manually grant Accessibility permissions to the glyphlow binary
              in System Settings > Privacy & Security > Accessibility for it to function.
            '';
            homepage = "https://github.com/blindFS/Glyphlow";
            license = licenses.mit;
            platforms = [ "aarch64-darwin" ];
          };
        };
      in
      {
        packages.default = glyphlow-bin;
        packages.glyphlow = glyphlow-bin;

        apps.default = flake-utils.lib.mkApp { drv = glyphlow-bin; };
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
              description = "The glyphlow package to use.";
            };
            settings = lib.mkOption {
              type = tomlFormat.type;
              default = { };
              description = "Configuration written to $XDG_CONFIG_HOME/glyphlow/config.toml.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];

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
              description = "The glyphlow package to use.";
            };
            settings = lib.mkOption {
              type = tomlFormat.type;
              default = { };
              description = "Configuration written to $XDG_CONFIG_HOME/glyphlow/config.toml.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            xdg.configFile."glyphlow/config.toml" = lib.mkIf (cfg.settings != { }) {
              source = tomlFormat.generate "glyphlow-config" cfg.settings;
            };

            # On macOS, home-manager can also manage launchd agents if using the macos module
            launchd.agents.glyphlow = lib.mkIf pkgs.stdenv.isDarwin {
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
