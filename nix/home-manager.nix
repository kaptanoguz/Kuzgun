self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.kuzgun;
  json = pkgs.formats.json { };
  # paths.rs: ProjectDirs::from("io.github", "kaptanoguz", "kuzgun").
  configDir =
    if pkgs.stdenv.hostPlatform.isDarwin then
      "${config.home.homeDirectory}/Library/Application Support/io.github.kaptanoguz.kuzgun"
    else
      "${config.xdg.configHome}/kuzgun";
  declared = json.generate "kuzgun-settings.json" cfg.settings;
  jq = lib.getExe pkgs.jq;
  readSecret = key: path: ''
    if [[ -r ${lib.escapeShellArg path} ]]; then
      kuzgunSecrets+=(--rawfile ${lib.escapeShellArg key} ${lib.escapeShellArg path})
    else
      warnEcho ${lib.escapeShellArg "Kuzgun: cannot read ${path}, keeping the current ${key}"}
    fi
  '';
in
{
  options.programs.kuzgun = {
    enable = lib.mkEnableOption "Kuzgun, a native WhatsApp client";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "kuzgun.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The Kuzgun package to install.";
    };

    settings = lib.mkOption {
      type = lib.types.attrsOf json.type;
      default = { };
      example = lib.literalExpression ''
        {
          check_for_updates = false;
          message_sound = "none";
          theme = "dark";
        }
      '';
      description = ''
        Keys merged into Kuzgun's `settings.json` on every activation. Keys
        left out keep whatever the app last saved, so window geometry, recent
        emoji and the lock verifiers survive. A key declared here is reset on
        the next activation if it is changed in the app. Per-account preferences
        such as read receipts live in each account's own file and cannot be
        declared here. Restart Kuzgun after
        activation: it reads the file at start, and its next save writes back
        the values it holds in memory.

        Values are copied to the Nix store, which every local user can read.
        Give secrets, such as proxy credentials or `giphy_key`, through
        {option}`programs.kuzgun.secretFiles` instead.
      '';
    };

    secretFiles = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = lib.literalExpression ''
        {
          proxy = config.sops.secrets.kuzgun-proxy.path;
        }
      '';
      description = ''
        Keys of `settings.json` whose string values are read at activation from
        these files, for example secrets decrypted by sops-nix or agenix, so
        they never enter the Nix store. Give each path as a string: a Nix path
        literal would be copied to the store. A trailing newline is dropped. A
        file that cannot be read leaves that key as it is, with a warning; a
        secret manager that decrypts after activation fills it on the next
        activation.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = builtins.all (key: !(cfg.settings ? ${key})) (builtins.attrNames cfg.secretFiles);
        message = "programs.kuzgun: a key cannot be in both settings and secretFiles.";
      }
    ];

    home.packages = [ cfg.package ];

    # settings.json cannot be a store symlink: the app replaces it whole on
    # every change and keeps its own state in it.
    home.activation.kuzgunSettings = lib.mkIf (cfg.settings != { } || cfg.secretFiles != { }) (
      lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        kuzgunDir=${lib.escapeShellArg configDir}
        kuzgunFile="$kuzgunDir/settings.json"
        run mkdir -p -m 700 "$kuzgunDir"
        if [[ ! -e "$kuzgunFile" ]]; then
          # A file without a version is migrated as one from older releases,
          # which would replace an explicit beige or black wallpaper.
          kuzgunBase='{"version":1}'
        elif kuzgunBase=$(${jq} -ce objects "$kuzgunFile" 2>/dev/null); then
          :
        else
          warnEcho "Kuzgun: $kuzgunFile is not a JSON object, leaving it unchanged"
          kuzgunBase=
        fi
        if [[ -z "$kuzgunBase" ]]; then
          :
        elif [[ -v DRY_RUN ]]; then
          echo "Would merge ${declared} into $kuzgunFile"
        else
          kuzgunSecrets=()
          ${lib.concatStrings (lib.mapAttrsToList readSecret cfg.secretFiles)}
          kuzgunTmp=$(mktemp "$kuzgunDir/.settings.json.XXXXXX")
          ${jq} --slurpfile __kuzgunDeclared ${declared} "''${kuzgunSecrets[@]}" \
            '. * $__kuzgunDeclared[0] * ($ARGS.named | del(.__kuzgunDeclared) | map_values(rtrimstr("\n")))' \
            <<< "$kuzgunBase" > "$kuzgunTmp"
          chmod 600 "$kuzgunTmp"
          mv "$kuzgunTmp" "$kuzgunFile"
        fi
        unset kuzgunDir kuzgunFile kuzgunBase kuzgunSecrets kuzgunTmp
      ''
    );
  };
}
