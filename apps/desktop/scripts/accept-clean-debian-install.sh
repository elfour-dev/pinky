#!/bin/sh
set -eu

package_path="$1"
package_name="$(dpkg-deb -f "$package_path" Package)"
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq "$package_path" xvfb dbus-x11
dpkg-query -W -f='${Package} ${Version} ${Architecture} ${Status}\n' "$package_name"
test -x /usr/bin/pinky-desktop
test -n "$(find /usr/share/applications -maxdepth 1 -iname '*pinky*.desktop' -print -quit)"
if ldd /usr/bin/pinky-desktop | grep -q 'not found'; then
  ldd /usr/bin/pinky-desktop
  exit 1
fi

useradd --create-home --user-group pinkytest
install -d -o pinkytest -g pinkytest /tmp/pinky-profile/config /tmp/pinky-profile/data /tmp/pinky-profile/cache
set +e
runuser -u pinkytest -- env \
  XDG_CONFIG_HOME=/tmp/pinky-profile/config \
  XDG_DATA_HOME=/tmp/pinky-profile/data \
  XDG_CACHE_HOME=/tmp/pinky-profile/cache \
  dbus-run-session -- xvfb-run -a timeout 5s /usr/bin/pinky-desktop
status=$?
set -e
if [ "$status" -ne 0 ] && [ "$status" -ne 124 ]; then
  echo "Pinky failed its clean-machine startup probe with status $status" >&2
  exit "$status"
fi

echo "Pinky clean Debian installation and startup accepted"
