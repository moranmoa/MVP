"""One WebKitGTK participant: the voice client in WebKit2 4.1, the engine
Tauri uses on Linux (wry -> webkit2gtk-4.1). Prints what the engine offers
and, when it has WebRTC, joins the room like any other peer and reports to
the signal server.

Env: SIGNAL=ws://signal:8080 ROOM DURATION SRC=pulses|mic NAME
"""

import json
import os
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2  # noqa: E402

env = os.environ
duration = int(env.get("DURATION", "40"))
name = env.get("NAME", "w-" + os.uname().nodename[:6])

print("WebKitGTK", WebKit2.get_major_version(), WebKit2.get_minor_version(), WebKit2.get_micro_version(), flush=True)

view = WebKit2.WebView()
settings = view.get_settings()
# Everything Tauri could switch on through wry's WebKit settings.
for prop in ["enable-media-stream", "enable-webrtc", "enable-webaudio", "enable-mediasource",
             "enable-mock-capture-devices", "enable-media", "media-playback-requires-user-gesture"]:
    if settings.find_property(prop) is None:
        print("setting missing:", prop, flush=True)
        continue
    settings.set_property(prop, prop != "media-playback-requires-user-gesture")
    print("setting", prop, "=", settings.get_property(prop), flush=True)


def on_permission(_view, request):
    print("permission request:", type(request).__name__, flush=True)
    request.allow()
    return True


view.connect("permission-request", on_permission)

win = Gtk.Window()
win.add(view)
win.show_all()


def js(code, then):
    def done(v, res):
        try:
            value = v.evaluate_javascript_finish(res)
            then(value.to_json(0) if value else None)
        except GLib.Error as e:
            then(json.dumps({"error": e.message}))

    view.evaluate_javascript(code, -1, None, None, None, done)


def finish():
    def got(report):
        print("REPORT", report, flush=True)
        Gtk.main_quit()

    js("JSON.stringify(window.__report || null)", got)
    return False


def caps():
    js("JSON.stringify({caps: window.__caps, log: document.getElementById('log').textContent})",
       lambda v: print("CAPS", v, flush=True))
    return False


url = (f"file:///voice/web/index.html?signal={env.get('SIGNAL', 'ws://signal:8080')}"
       f"&room={env.get('ROOM', 'bench')}&name={name}&src={env.get('SRC', 'pulses')}"
       f"&autojoin=1&duration={duration}")
view.load_uri(url)
GLib.timeout_add_seconds(5, caps)
GLib.timeout_add_seconds(duration + 8, finish)
GLib.timeout_add_seconds(duration + 30, lambda: sys.exit(1))
Gtk.main()
