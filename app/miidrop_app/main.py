import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gtk  # noqa: E402

from . import APP_ID


class Window(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Miidrop", default_width=900, default_height=640)
        view = Adw.ToolbarView()
        view.add_top_bar(Adw.HeaderBar())
        view.set_content(Adw.StatusPage(title="Miidrop", icon_name="network-wireless-symbolic"))
        self.set_content(view)


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id=APP_ID)

    def do_activate(self):
        (self.props.active_window or Window(self)).present()


def main():
    return App().run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
