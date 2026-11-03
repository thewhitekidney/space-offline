#!/usr/bin/env python3
"""Make a client CI run silent and cheap, for that run only.

    quiet-settings.py apply   DRIVE_C CLIENT_DIR
    quiet-settings.py restore DRIVE_C CLIENT_DIR

An unattended client has no desktop audio, so Wine falls back to ALSA and opens
the sound card directly: the desktop mixer never sees it and it plays through
the owner's speakers. So CI turns the client's own audio off, and sets the
lowest graphics while it is there.

`core_public__.yaml` is the owner's own settings file for that client, and a
human launch reads it too. `apply` keeps a copy and `restore` puts it back. A
run that died leaves the copy behind, and the next `apply` restores it first,
so the kept copy is always the owner's file and never a forced one.

The file is edited as text: the client writes one flat `  key: [stamp, value]`
line per setting, and the tools have no YAML parser (std only).
"""
import glob
import os
import shutil
import sys

SETTINGS = "core_public__.yaml"
KEEP = SETTINGS + ".ci-keep"
NONE = SETTINGS + ".ci-none"  # there was no file to keep
STAMP = 134379241243113623

# Values from evegraphics/settings.py: its low-end device preset, the off end
# of each scale beyond that preset, and gfxsettings' texture levels.
FORCED = {
    "audio": {
        "audioEnabled": "0",
        "masterVolume": "0.0",
        "uiGain": "0.0",
        "worldVolume": "0.0",
        "eveampGain": "0.0",
        "evevoiceGain": "0.0",
        "useCombatMusic": "0",
    },
    "device": {
        "antiAliasing": "0",
        "aoQuality": "0",
        "charClothSimulation": "0",
        "charTextureQuality": "2",
        "dofEnabled": "false",
        "fastCharacterCreation": "1",
        "frameGeneration": "false",
        "lodQuality": "1",
        "postProcessingQuality": "0",
        "reflectionQuality": "0",
        "shaderQuality": "1",
        "shadowQuality": "0",
        "textureQuality": "2",
        "upscalingTechnique": "0",
        "volumetricQuality": "0",
    },
}


def force(text):
    """`text` with every FORCED key set, any other line unchanged."""
    lines = text.splitlines()
    for section, keys in FORCED.items():
        rows = ["  %s: [%d, %s]" % (k, STAMP, v) for k, v in keys.items()]
        head = next((i for i, l in enumerate(lines) if l.split(":")[0] == section), None)
        if head is None:
            lines += [section + ":"] + rows
            continue
        lines[head] = section + ":"  # `audio: {}` is how an empty one looks
        end = head + 1
        while end < len(lines) and lines[end][:1] in (" ", "-"):
            end += 1
        body, i = [], head + 1
        while i < end:
            key = lines[i].strip().split(":")[0]
            skip = lines[i].startswith("  ") and key in keys
            if skip:
                i += 1  # a value may run on over deeper or `- ` lines
                while i < end and not (lines[i].startswith("  ") and lines[i][2] not in " -"):
                    i += 1
                continue
            body.append(lines[i])
            i += 1
        lines[head + 1 : end] = body + rows
    return "\n".join(lines) + "\n"


def settings_dirs(drive_c, client_dir):
    """Every settings folder of the install, or the one the client will make."""
    root = os.path.join(drive_c, "users", "steamuser", "AppData", "Local", "CCP", "EVE")
    folder = "c_ccp_eve_" + client_dir
    found = sorted(glob.glob(os.path.join(glob.escape(root), glob.escape(folder) + "_*", "settings*")))
    found = [d for d in found if os.path.isdir(d)]
    return found or [os.path.join(root, folder + "_tranquility", "settings")]


def restore(directory):
    path = os.path.join(directory, SETTINGS)
    keep = os.path.join(directory, KEEP)
    none = os.path.join(directory, NONE)
    if os.path.exists(keep):
        os.replace(keep, path)
    elif os.path.exists(none):
        os.remove(none)
        if os.path.exists(path):
            os.remove(path)


def apply(directory):
    restore(directory)  # a dead run's forced file must not become the kept one
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, SETTINGS)
    if os.path.exists(path):
        shutil.copy2(path, os.path.join(directory, KEEP))
        with open(path, encoding="utf-8") as f:
            text = f.read()
    else:
        open(os.path.join(directory, NONE), "w").close()
        text = ""
    with open(path, "w", encoding="utf-8") as f:
        f.write(force(text))


def main(argv):
    if len(argv) != 4 or argv[1] not in ("apply", "restore"):
        sys.exit(__doc__)
    for directory in settings_dirs(argv[2], argv[3]):
        (apply if argv[1] == "apply" else restore)(directory)
        print("   quiet settings: %s %s" % (argv[1], directory))


if __name__ == "__main__":
    main(sys.argv)
