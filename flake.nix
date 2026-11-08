{
  description = "pushover-mcp-rs: MCP server for sending push notifications via the Pushover API";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      # Name, version, description and repository come from Cargo.toml, so a
      # release only ever touches one file (set-version.sh sets the version there).
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      # Shared per-system setup, evaluated once and used by packages, devShells and checks
      perSystem = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain = pkgs.rust-bin.stable.latest.default;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

          # crane's default filter keeps the Cargo sources; the tests need
          # nothing else.
          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          inherit pkgs toolchain craneLib commonArgs cargoArtifacts;
        };
    in
    {
      packages = forAllSystems (system:
        let
          s = perSystem system;
          # crane runs `cargo test` while building the package; the tests
          # talk to a fake Pushover API on 127.0.0.1, not to the network.
          pkg = s.craneLib.buildPackage (s.commonArgs // {
            inherit (s) cargoArtifacts;
            meta = {
              description = cargoToml.package.description;
              homepage = cargoToml.package.repository;
              license = s.pkgs.lib.licenses.mit;
              mainProgram = cargoToml.package.name;
            };
          });
        in
        {
          default = pkg;
          pushover-mcp-rs = pkg;
        }
      );

      checks = forAllSystems (system:
        let
          s = perSystem system;
        in
        {
          # Build
          package = self.packages.${system}.default;

          # Unit tests and the stdio tests against the fake API
          test = s.craneLib.cargoTest (s.commonArgs // {
            inherit (s) cargoArtifacts;
          });

          # Clippy on all targets, warnings denied
          clippy = s.craneLib.cargoClippy (s.commonArgs // {
            inherit (s) cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });

          # Formatting
          fmt = s.craneLib.cargoFmt s.commonArgs;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/${cargoToml.package.name}";
          meta.description = cargoToml.package.description;
        };
      });

      devShells = forAllSystems (system:
        let
          s = perSystem system;
          devToolchain = s.toolchain.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          };
        in
        {
          default = s.pkgs.mkShell {
            nativeBuildInputs = [
              devToolchain
              # Known vulnerabilities and unused crates in the dependencies
              s.pkgs.cargo-audit
              s.pkgs.cargo-machete
            ];
          };
        }
      );
    };
}
