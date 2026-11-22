import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gio, GLib, Gtk

from . import APP_ID
from .connect import ConnectPage
from .helper import Helper
from .library_page import LibraryPage
from .preferences import PreferencesDialog
from .render import Renderer
from .settings import Settings

# pkexec exits with these when the password prompt is dismissed or refused.
DENIED = {126, 127}


class Window(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Miidrop", default_width=720, default_height=680)
        self.settings = Settings.load()
        self.helper = Helper(lambda e: GLib.idle_add(self.on_event, e))
        self.helper_interface = None
        self._renderer = None
        self.beacon_key = False
        self.state = "idle"

        self.toasts = Adw.ToastOverlay()
        stack = Adw.ViewStack()
        self.connect_page = ConnectPage(self)
        self.library_page = LibraryPage(self)
        stack.add_titled_with_icon(self.connect_page, "connect", "Connect", "network-wireless-symbolic")
        stack.add_titled_with_icon(self.library_page, "library", "Library", "avatar-default-symbolic")

        header = Adw.HeaderBar(title_widget=Adw.ViewSwitcher(stack=stack, policy=Adw.ViewSwitcherPolicy.WIDE))
        menu = Gio.Menu()
        menu.append("Preferences", "app.preferences")
        menu.append("About Miidrop", "app.about")
        header.pack_end(Gtk.MenuButton(icon_name="open-menu-symbolic", menu_model=menu, primary=True))

        view = Adw.ToolbarView()
        view.add_top_bar(header)
        view.set_content(stack)
        bar = Adw.ViewSwitcherBar(stack=stack)
        view.add_bottom_bar(bar)
        self.toasts.set_child(view)
        self.set_content(self.toasts)

        narrow = Adw.Breakpoint.new(Adw.BreakpointCondition.parse("max-width: 500sp"))
        narrow.add_setter(bar, "reveal", True)
        self.add_breakpoint(narrow)

        self.library_page.refresh()
        self.connect("close-request", self.on_close)

    @property
    def renderer(self):
        """The picture renderer, or None when pictures are turned off."""
        if not self.settings.pictures:
            return None
        if self._renderer is None or self._renderer.server != self.settings.picture_server:
            if self._renderer:
                self._renderer.close()
            self._renderer = Renderer(self.settings.picture_server)
        return self._renderer

    def toast(self, text, action=None):
        t = Adw.Toast(title=GLib.markup_escape_text(text), timeout=4)
        if action:
            t.set_button_label(action[0])
            t.set_action_name(action[1])
        self.toasts.add_toast(t)

    # Helper lifecycle

    def ensure_helper(self, needs_keys=True):
        keys = self.settings.keys_text()
        if needs_keys and keys is None:
            self.toast("Choose your key file first", ("Preferences", "app.preferences"))
            return False
        interface = self.connect_page.interface()
        if not interface:
            self.toast("No wireless card found")
            return False
        if self.helper.running and self.helper_interface != interface:
            self.helper.close()
        if not self.helper.running:
            try:
                self.helper.start(interface)
            except OSError as e:
                self.toast(f"Could not start the helper: {e}")
                return False
            self.helper_interface = interface
            self.settings.interface = interface
            self.settings.save()
            if keys is not None:
                self.helper.send_keys(keys)
        return True

    def command(self, method, *args, needs_keys=True, **kwargs):
        if self.ensure_helper(needs_keys):
            try:
                getattr(self.helper, method)(*args, **kwargs)
            except (RuntimeError, OSError) as e:
                self.toast(str(e))

    def scan(self):
        self.command("scan", needs_keys=False)

    def join(self, host=None):
        self.command("join", host=host, name=self.settings.name)

    def host(self):
        self.command("host", name=self.settings.name)

    def stop(self):
        if self.helper.running:
            self.helper.stop()

    def can_send(self):
        return self.state == "ready"

    def send_record(self, record):
        if not self.can_send():
            self.toast("Not connected to a console")
            return
        issues = record.issues()
        if issues:
            self.toast(f"{record.name} would be refused: {issues[0]}")
            return
        self.helper.send_mii(bytes(record))

    def on_close(self, _window):
        self.helper.close()
        if self._renderer:
            self._renderer.close()
        return False

    # Events from the helper

    def on_event(self, e):
        kind = e["event"]
        page = self.connect_page
        if kind == "hello":
            if not e.get("active_monitor"):
                self.toast("This card cannot ACK frames, transfers may fail")
        elif kind == "keys":
            self.beacon_key = e["beacon"]
        elif kind == "state":
            self.state = e["state"]
            page.set_state(self.state)
        elif kind == "found":
            page.found(e["console"])
        elif kind == "joining":
            page.status("Joining…")
        elif kind == "hosting":
            if not e.get("beacon_key"):
                self.toast("Without 'udsBeaconKey' the console will list no players")
        elif kind == "connected":
            page.connected(e.get("peer"), e.get("role"))
        elif kind == "ready":
            self.state = "ready"
            page.ready()
        elif kind == "sending":
            page.status(f"Sending {e.get('name')}…")
        elif kind == "peer_receiving":
            page.status(f"{page.peer} is receiving…")
        elif kind == "verdict":
            text = f"{page.peer} saved it" if e["accepted"] else f"{page.peer} refused it"
            page.status(text)
            self.toast(text)
        elif kind == "incoming":
            page.status(f"{page.peer} is sending a Mii…")
        elif kind == "received":
            page.add_received(e)
            page.status("Ready")
        elif kind == "left":
            self.toast(f"{page.peer} left")
        elif kind == "error":
            self.toast(e.get("message", "Unknown error"))
        elif kind == "exit":
            self.state = "idle"
            page.set_state("idle")
            code = e.get("code")
            if code in DENIED:
                self.toast("Miidrop was not given access to the wireless card")
            elif code:
                lines = [l for l in e.get("stderr", "").splitlines() if l.strip()]
                self.toast(lines[-1] if lines else f"The helper stopped (code {code})")
        return GLib.SOURCE_REMOVE


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id=APP_ID)
        for name, callback in (("preferences", self.preferences), ("about", self.about), ("quit", self.quit_app)):
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", callback)
            self.add_action(action)
        self.set_accels_for_action("app.preferences", ["<Control>comma"])
        self.set_accels_for_action("app.quit", ["<Control>q"])

    def do_activate(self):
        (self.props.active_window or Window(self)).present()

    def preferences(self, *_):
        window = self.props.active_window
        if window:
            PreferencesDialog(window).present(window)

    def about(self, *_):
        Adw.AboutDialog(
            application_name="Miidrop",
            application_icon="network-wireless-symbolic",
            license_type=Gtk.License.MIT_X11,
            comments="Send and recieve Miis wirelessly from a 3DS or Wii U.",
        ).present(self.props.active_window)

    def quit_app(self, *_):
        window = self.props.active_window
        if window:
            window.close()
        self.quit()


def main():
    return App().run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
