{ lib
, rustPlatform
, bubblewrap
, coreutils
, nix
, passt
, systemd
, makeWrapper
, clippy
, rustfmt
}:

rustPlatform.buildRustPackage {
  pname = "realm";
  version = "0.1.0";
  src = lib.cleanSourceWith {
    filter = path: type: !(type == "directory" && baseNameOf path == "target");
    src = ./.;
  };
  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ clippy makeWrapper rustfmt ];
  doCheck = true;

  checkPhase = ''
    runHook preCheck
    export PATH=${lib.makeBinPath [ bubblewrap coreutils nix passt systemd ]}:$PATH
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked --lib
    cargo test --locked --test environment --test mounts --test network_modes -- --test-threads=1
    runHook postCheck
  '';

  postInstall = ''
    wrapProgram $out/bin/realm --prefix PATH : ${lib.makeBinPath [ bubblewrap coreutils nix passt systemd ]}
  '';

  meta = {
    description = "Nix-native sandbox runtime for agent workloads";
    mainProgram = "realm";
    platforms = lib.platforms.linux;
  };
}
