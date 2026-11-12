#!/usr/bin/python3
"""Start blueman-manager with Escape closing its main window."""

import gettext
import logging

import gi

gi.require_version("Gdk", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import GLib  # noqa: E402

GLib.set_prgname("blueman-manager")

from blueman.Functions import create_logger, create_parser, set_proc_title  # noqa: E402
from blueman.main.Manager import Blueman  # noqa: E402
from gi.repository import Gdk, Gtk  # noqa: E402

LOG_LEVELS = {
    name: getattr(logging, name)
    for name in ("DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL")
}


def close_on_escape(_window, event, app):
    modifiers = event.state & Gtk.accelerator_get_default_mod_mask()
    if event.keyval != Gdk.KEY_Escape or modifiers:
        return False
    app.quit()
    return True


def bind_escape(app):
    if app.window is None:
        return
    app.window.connect("key-press-event", close_on_escape, app)
    app.disconnect_by_func(bind_escape)


def main():
    gettext.textdomain("blueman")
    args = create_parser().parse_args()
    level = LOG_LEVELS.get(args.LEVEL.upper(), logging.WARNING)
    create_logger(level, "blueman-manager", syslog=args.syslog)

    app = Blueman()
    app.connect_after("activate", bind_escape)
    set_proc_title("blueman-manager")
    return app.run()


if __name__ == "__main__":
    raise SystemExit(main())
