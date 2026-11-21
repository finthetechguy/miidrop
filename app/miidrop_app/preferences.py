"""The preferences dialog: key file, library folder and the name consoles see."""

import miidrop
from gi.repository import Adw, GLib, Gtk


class PreferencesDialog(Adw.PreferencesDialog):
    def __init__(self, window):
        super().__init__(title="Preferences")
        self.window = window
        self.settings = window.settings
        page = Adw.PreferencesPage()
        self.add(page)

        keys = Adw.PreferencesGroup(
            title="Keys",
            description="An 'aes_keys.txt' dumped from your own console.",
        )
        self.keys_row = Adw.ActionRow(title="Key file")
        self.keys_row.add_suffix(self._choose_button(self.choose_keys))
        keys.add(self.keys_row)
        page.add(keys)

        library = Adw.PreferencesGroup(title="Library")
        self.library_row = Adw.ActionRow(title="Folder")
        self.library_row.add_suffix(self._choose_button(self.choose_library))
        library.add(self.library_row)
        page.add(library)

        you = Adw.PreferencesGroup(title="Connection")
        self.name_row = Adw.EntryRow(title="Name shown to consoles", max_length=10, text=self.settings.name)
        self.name_row.connect("changed", self.name_changed)
        you.add(self.name_row)
        page.add(you)
        self.update()

    @staticmethod
    def _choose_button(callback):
        button = Gtk.Button(label="Choose…", valign=Gtk.Align.CENTER)
        button.connect("clicked", lambda _b: callback())
        return button

    def update(self):
        text = self.settings.keys_text()
        if not self.settings.keys_path:
            status = "Not set"
        elif text is None:
            status = "Cannot be read"
        else:
            try:
                status = "Ready to join and host" if miidrop.check_keys(text) else "Ready to join; hosting needs 'udsBeaconKey'"
            except ValueError as e:
                status = str(e)
        self.keys_row.set_subtitle(GLib.markup_escape_text(f"{self.settings.keys_path or '—'}\n{status}"))
        self.library_row.set_subtitle(GLib.markup_escape_text(self.settings.library))

    def choose_keys(self):
        def done(d, result):
            try:
                self.settings.keys_path = d.open_finish(result).get_path()
            except GLib.Error:
                return
            self.saved()

        Gtk.FileDialog(title="Choose key file").open(self.window, None, done)

    def choose_library(self):
        def done(d, result):
            try:
                self.settings.library = d.select_folder_finish(result).get_path()
            except GLib.Error:
                return
            self.saved()
            self.window.library_page.refresh()

        Gtk.FileDialog(title="Choose library folder").select_folder(self.window, None, done)

    def name_changed(self, row):
        name = row.get_text().strip()
        if name:
            self.settings.name = name
            self.settings.save()

    def saved(self):
        self.settings.save()
        self.update()
