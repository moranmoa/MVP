#!/bin/bash
# T12 install / uninstall check for one shell's Linux packages (.deb and
# AppImage), in a clean Debian container (run as root).
#
#   packaging/install-tests/linux.sh tauri out/
#
# .deb (needs root): install, first launch as a normal user, install again
# over it, apt remove, apt purge, reinstall. AppImage (per-user, no root):
# first launch as a normal user, then delete it. After every uninstall route
# the user's data folder must still be there (INS-10: purge never touches
# $HOME). Every before/after diff is printed for QA's records.
set -uo pipefail
shell=$1
dir=$(cd "$2" && pwd)

case "$shell" in
  tauri)
    pkg=shell-spike-tauri
    bin=/usr/bin/shell-spike-tauri
    data=.local/share/dev.mvp.shellspike.tauri
    installed="/usr/bin/shell-spike-tauri /usr/share/applications/ShellSpikeTauri.desktop"
    launch_flags=""
    ;;
  electron)
    pkg=shell-spike-electron
    bin=/usr/bin/shell-spike-electron
    data=.config/shell-spike-electron
    installed="/opt/ShellSpikeElectron /usr/share/applications/shell-spike-electron.desktop /usr/bin/shell-spike-electron"
    # The container has no unprivileged user namespaces for Chromium's sandbox.
    launch_flags="--no-sandbox"
    ;;
  *) echo "usage: $0 tauri|electron <package dir>"; exit 2 ;;
esac
deb="$dir/shell-$shell-linux-amd64.deb"
appimage="$dir/shell-$shell-linux-amd64.AppImage"
user=tester
uhome=/home/$user

# A fresh container needs a virtual display and a normal user.
if ! id "$user" >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq && apt-get install -y -qq --no-install-recommends xvfb xauth procps ca-certificates >/dev/null
  useradd -m "$user"
fi

fails=0
check() { # condition-exit-code description
  if [ "$1" -eq 0 ]; then echo "  ok    $2"; else echo "  FAIL  $2"; fails=$((fails + 1)); fi
}
# Everything a package can add system-wide or for the user.
snapshot() {
  {
    find /usr/bin /usr/local/bin /opt /usr/share/applications /usr/share/icons /etc/xdg/autostart \
      /usr/lib/systemd -mindepth 1 -maxdepth 1 2>/dev/null
    find "$uhome/.config" "$uhome/.local/share" "$uhome/.local/share/applications" "$uhome/.config/autostart" \
      -mindepth 1 -maxdepth 1 2>/dev/null
    dpkg-query -W -f='dpkg:${Package} ${db:Status-Status}\n' "$pkg" 2>/dev/null | grep ' installed$'
    grep -h '^x-scheme-handler/mvs' /usr/share/applications/mimeinfo.cache "$uhome/.config/mimeapps.list" 2>/dev/null |
      sed 's/^/mime:/'
  } | sort -u
}
diff_from() { # before-file label -> prints +/- lines
  snapshot > /tmp/now
  echo "  diff $2"
  comm -13 "$1" /tmp/now | sed 's/^/    + /'
  comm -23 "$1" /tmp/now | sed 's/^/    - /'
}
added() { comm -13 "$1" <(snapshot); }
launch() { # program -> 0 if still running after 10 s
  su "$user" -c "cd ~ && timeout 10 xvfb-run -a $1 $launch_flags >/tmp/launch.log 2>&1"
  local code=$?
  [ "$code" -eq 124 ] || { echo "    exit $code:"; tail -5 /tmp/launch.log | sed 's/^/    /'; }
  [ "$code" -eq 124 ]
}
leftover() { pgrep -f "$pkg" >/dev/null && echo yes || echo no; }
apt_do() { # apt-get args... (prints its log on failure)
  apt-get -y -qq "$@" >/tmp/apt.log 2>&1 || { sed "s/^/    /" /tmp/apt.log | tail -8; return 1; }
}

echo "== $shell (.deb)"
# Install and purge once so its dependencies (webkit, gtk, ...) are already on
# the machine: the diffs below then show only what the package itself adds.
apt-get install -y -qq "$deb" >/tmp/apt.log 2>&1 && apt-get purge -y -qq "$pkg" >>/tmp/apt.log 2>&1 ||
  { cat /tmp/apt.log; echo "FAIL: cannot install $deb"; exit 1; }
snapshot > /tmp/clean

echo "1. apt install"
apt_do install "$deb"; check $? "apt install ./$(basename "$deb")"
diff_from /tmp/clean 'clean -> installed'
for f in $installed; do test -e "$f"; check $? "installed $f"; done
added /tmp/clean | grep -q -e autostart -e systemd; check $((1 - $?)) 'no autostart or service'
snapshot > /tmp/installed

echo "2. first launch (user $user)"
launch "$bin"; check $? 'app still running after 10 s'
test -d "$uhome/$data"; check $? "data folder created: ~/$data"
snapshot > /tmp/launched

echo "3. install again over the existing install"
apt_do install --reinstall "$deb"; check $? 'apt install --reinstall'
diff_from /tmp/launched 'installed -> installed again'
[ -z "$(comm -3 /tmp/launched <(snapshot))" ]; check $? 'no duplicate entries, nothing lost'

echo "4. apt remove"
apt_do remove "$pkg"; check $? 'apt remove'
diff_from /tmp/clean 'clean -> removed'
for f in $installed; do test ! -e "$f"; check $? "removed $f"; done
test -d "$uhome/$data"; check $? 'user data kept after remove'
[ "$(leftover)" = no ]; check $? 'no process left running'

echo "5. apt purge"
# dpkg, not apt: a package without conffiles is already forgotten after remove.
dpkg --purge "$pkg" >/tmp/apt.log 2>&1; check $? 'purge (dpkg --purge)'
test -d "$uhome/$data"; check $? 'user data kept after purge (home is never touched)'
[ -z "$(added /tmp/clean | grep -v -e "^$uhome/$data$" -e "/.local/share/pki$")" ]; check $? 'only the user data folder is left'

echo "6. reinstall after removal"
apt_do install "$deb"; check $? 'apt install again'
launch "$bin"; check $? 'app starts again with the old data folder'
apt-get purge -y -qq "$pkg" >/dev/null 2>&1

echo "== $shell (AppImage, per-user, no root)"
rm -rf "${uhome:?}/$data"
snapshot > /tmp/clean
install -o "$user" -m 0755 "$appimage" "$uhome/app.AppImage"
# No FUSE in a container; real desktops mount it instead.
export APPIMAGE_EXTRACT_AND_RUN=1
launch "env APPIMAGE_EXTRACT_AND_RUN=1 ~/app.AppImage"; check $? 'AppImage runs as a normal user, no install'
test -d "$uhome/$data"; check $? "data folder created: ~/$data"
diff_from /tmp/clean 'clean -> after AppImage launch'
added /tmp/clean | grep -v -e "^$uhome/app.AppImage$" -e "^$uhome/$data$" -e '/.local/share/pki$' -e '/.cache' |
  grep -q .; check $((1 - $?)) 'AppImage added nothing else (no .desktop, no mvs:// handler)'
rm -f "$uhome/app.AppImage"
test -d "$uhome/$data"; check $? 'deleting the AppImage leaves the user data'

echo
if [ "$fails" -gt 0 ]; then echo "$shell: $fails check(s) failed"; exit 1; fi
echo "$shell: all checks passed"
