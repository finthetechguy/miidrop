"""The page for finding a console and exchanging Miis with it."""

import miidrop
from gi.repository import Adw, GLib, Gtk

from .details import DetailsDialog
from .interfaces import wireless_interfaces
from .picture import mii_avatar

BUSY = {
    "searching": ("Looking for a console", "Choose 'Wait for Players' in Mii Maker on the console."),
    "hosting": ("Waiting for a console", "Choose 'Search for Players' in Mii Maker on the console, then pick this PC."),
}


def esc(text):
    return GLib.markup_escape_text(str(text))


def button(label, callback, style=None):
    b = Gtk.Button(label=label, valign=Gtk.Align.CENTER)
    if style:
        b.add_css_class(style)
    b.connect("clicked", lambda _b: callback())
    return b


class ConnectPage(Gtk.Stack):
    def __init__(self, window):
        super().__init__(transition_type=Gtk.StackTransitionType.CROSSFADE, vexpand=True)
        self.window = window
        self.add_named(self._setup(), "setup")
        self.add_named(self._busy(), "busy")
        self.add_named(self._session(), "session")
        self.found_rows = {}
        self.received_rows = []
        self.peer = ""
        self.refresh_interfaces()

    # Pages

    def _setup(self):
        page = Adw.PreferencesPage()
        card = Adw.PreferencesGroup(title="Wireless card", description="The card stops being used for Wi-Fi while using Miidrop.")
        self.interfaces = Gtk.StringList()
        self.iface_row = Adw.ComboRow(title="Card", model=self.interfaces)
        card.add(self.iface_row)
        refresh = Gtk.Button(icon_name="view-refresh-symbolic", tooltip_text="Refresh", valign=Gtk.Align.CENTER)
        refresh.add_css_class("flat")
        refresh.connect("clicked", lambda _b: self.refresh_interfaces())
        card.set_header_suffix(refresh)
        page.add(card)

        start = Adw.PreferencesGroup(title="Connect")
        join = Adw.ActionRow(title="Join a console", subtitle="Select 'Wait for Players' on 3DS or join from Wii U")
        self.join_button = button("Join", lambda: self.window.join(), "suggested-action")
        join.add_suffix(self.join_button)
        start.add(join)
        host = Adw.ActionRow(title="Host", subtitle="Select 'Search for Players' on 3DS")
        self.host_button = button("Host", lambda: self.window.host())
        host.add_suffix(self.host_button)
        start.add(host)
        page.add(start)

        self.nearby = Adw.PreferencesGroup(title="Nearby consoles", description="Consoles in Mii Maker that are waiting for players.")
        self.scan_button = button("Scan", self.toggle_scan)
        self.scan_button.add_css_class("flat")
        self.nearby.set_header_suffix(self.scan_button)
        page.add(self.nearby)
        return page

    def _busy(self):
        self.busy_page = Adw.StatusPage(paintable=Adw.SpinnerPaintable(widget=self))
        cancel = Gtk.Button(label="Cancel", halign=Gtk.Align.CENTER)
        cancel.add_css_class("pill")
        cancel.connect("clicked", lambda _b: self.window.stop())
        self.busy_page.set_child(cancel)
        return self.busy_page

    def _session(self):
        page = Adw.PreferencesPage()
        self.peer_group = Adw.PreferencesGroup()
        self.disconnect = button("Disconnect", self.leave_session, "destructive-action")
        self.peer_group.set_header_suffix(self.disconnect)
        page.add(self.peer_group)

        send = Adw.PreferencesGroup(title="Send")
        self.send_row = Adw.ActionRow(title="Send a Mii")
        self.send_button = button("Choose…", self.choose_mii, "suggested-action")
        self.send_row.add_suffix(self.send_button)
        send.add(self.send_row)
        page.add(send)

        self.received = Adw.PreferencesGroup(title="Received", description="Miis the console sends appear here.")
        page.add(self.received)
        return page

    # Card list

    def refresh_interfaces(self):
        cards = wireless_interfaces()
        self.interfaces.splice(0, self.interfaces.get_n_items(), [n + (" (connected)" if c else "") for n, c in cards])
        self.cards = [n for n, _c in cards]
        saved = self.window.settings.interface
        if saved in self.cards:
            self.iface_row.set_selected(self.cards.index(saved))
        sensitive = bool(self.cards)
        for b in (self.join_button, self.host_button, self.scan_button):
            b.set_sensitive(sensitive)
        self.iface_row.set_subtitle("" if sensitive else "No wireless card found")

    def interface(self):
        i = self.iface_row.get_selected()
        return self.cards[i] if 0 <= i < len(self.cards) else None

    # State from the helper

    def set_state(self, state):
        scanning = state == "scanning"
        self.scan_button.set_label("Stop" if scanning else "Scan")
        for b in (self.join_button, self.host_button):
            b.set_sensitive(not scanning and bool(self.cards))
        self.iface_row.set_sensitive(not scanning)
        if state in BUSY:
            title, description = BUSY[state]
            self.busy_page.set_title(title)
            self.busy_page.set_description(description)
            self.set_visible_child_name("busy")
        elif state in ("connected", "ready"):
            self.set_visible_child_name("session")
        elif state == "idle" and self.get_visible_child_name() != "setup":
            if self.received_rows:
                # Unsaved Miis stay listed until the user decides.
                self.peer_group.set_title(esc(f"{self.peer} disconnected"))
                self.peer_group.set_description("Save or discard what was sent.")
                self.send_button.set_sensitive(False)
                self.send_row.set_subtitle("")
                self.disconnect.set_label("Done")
                self.disconnect.remove_css_class("destructive-action")
                self.set_visible_child_name("session")
            else:
                self.set_visible_child_name("setup")

    def status(self, text):
        if self.get_visible_child_name() == "busy":
            self.busy_page.set_title(text)
        else:
            self.send_row.set_subtitle(esc(text))

    def toggle_scan(self):
        if self.scan_button.get_label() == "Stop":
            self.window.stop()
        else:
            for row in self.found_rows.values():
                self.nearby.remove(row)
            self.found_rows = {}
            self.window.scan()

    def found(self, console):
        host = console["host"]
        names = [n for n in console.get("names") or [] if n]
        title = names[0] if names else host
        total, most = console.get("nodes", [0, 0])
        if console.get("available") and total < most:
            subtitle = f"Waiting for players · channel {console['channel']}"
        else:
            subtitle = "Busy"
        row = self.found_rows.get(host)
        if row is None:
            row = Adw.ActionRow()
            join = button("Join", lambda h=host: self.window.join(h))
            row.add_suffix(join)
            row.join = join
            self.nearby.add(row)
            self.found_rows[host] = row
        row.set_title(esc(title))
        row.set_subtitle(esc(subtitle))
        row.join.set_sensitive(subtitle != "Busy")

    def leave_session(self):
        if self.disconnect.get_label() == "Done":
            self.clear_received()
            self.set_visible_child_name("setup")
        else:
            self.window.stop()

    def connected(self, peer, role):
        self.clear_received()
        self.disconnect.set_label("Disconnect")
        self.disconnect.add_css_class("destructive-action")
        self.peer = peer or "the console"
        self.peer_group.set_title(esc(f"Connected to {self.peer}"))
        self.peer_group.set_description("This PC is hosting." if role == "host" else "This PC joined the console.")
        self.send_button.set_sensitive(False)
        self.send_row.set_subtitle("Waiting for the console to be ready…")

    def ready(self):
        self.send_button.set_sensitive(True)
        self.send_row.set_subtitle("Ready")

    # Sending and receiving

    def choose_mii(self):
        entries = self.window.library_page.library.entries()
        if not entries:
            self.window.toast("The library is empty; import a Mii first")
            return
        dialog = Adw.Dialog(title="Send a Mii", content_width=380, content_height=480)
        view = Adw.ToolbarView()
        view.add_top_bar(Adw.HeaderBar())
        page = Adw.PreferencesPage()
        group = Adw.PreferencesGroup()
        for _path, record in entries:
            row = Adw.ActionRow(title=esc(record.name or "Unnamed"), activatable=True)
            row.add_prefix(mii_avatar(record, 40, self.window.renderer))
            if record.creator:
                row.set_subtitle(esc(f"by {record.creator}"))
            if record.issues():
                row.add_suffix(Gtk.Image(icon_name="dialog-warning-symbolic", tooltip_text="A console would refuse it"))
            row.connect("activated", lambda _r, rec=record: (dialog.close(), self.window.send_record(rec)))
            group.add(row)
        page.add(group)
        view.set_content(page)
        dialog.set_child(view)
        dialog.present(self.window)

    def add_received(self, event):
        if not event.get("record"):
            self.window.toast(f"The Mii {self.peer} sent could not be read: {event.get('error')}")
            return
        record = miidrop.Record(bytes.fromhex(event["record"]))
        row = Adw.ActionRow(title=esc(record.name or "Unnamed"), activatable=True)
        row.add_prefix(mii_avatar(record, 40, self.window.renderer))
        row.set_subtitle(esc(f"by {record.creator}" if record.creator else ""))
        save = button("Save", lambda: self.save(row, record), "suggested-action")
        discard = Gtk.Button(icon_name="user-trash-symbolic", tooltip_text="Discard", valign=Gtk.Align.CENTER)
        discard.add_css_class("flat")
        discard.connect("clicked", lambda _b: self.remove_received(row))
        row.add_suffix(save)
        row.add_suffix(discard)
        row.connect("activated", lambda _r: self.show_received(row, record))
        self.received.add(row)
        self.received_rows.append(row)

    def show_received(self, row, record):
        dialog = DetailsDialog(record, renderer=self.window.renderer)
        dialog.add_action("Save", lambda: (dialog.close(), self.save(row, record)), "suggested-action")
        dialog.add_action("Discard", lambda: (dialog.close(), self.remove_received(row)))
        dialog.present(self.window)

    def save(self, row, record):
        try:
            self.window.library_page.library.add(record)
        except OSError as e:
            self.window.toast(str(e))
            return
        self.window.library_page.refresh()
        self.window.toast(f"Saved {record.name}")
        self.remove_received(row)

    def remove_received(self, row):
        if row in self.received_rows:
            self.received_rows.remove(row)
            self.received.remove(row)
        if not self.received_rows and self.disconnect.get_label() == "Done":
            self.set_visible_child_name("setup")

    def clear_received(self):
        for row in self.received_rows:
            self.received.remove(row)
        self.received_rows = []
