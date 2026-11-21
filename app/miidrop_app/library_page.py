"""The page listing saved Miis."""

from gi.repository import Adw, Gio, GLib, Gtk

from .details import DetailsDialog
from .library import SUFFIX, Library


def cfsd_filter():
    f = Gtk.FileFilter(name="Mii files")
    for pattern in ("*.cfsd", "*.bin", "*.mii"):
        f.add_pattern(pattern)
    filters = Gio.ListStore.new(Gtk.FileFilter)
    filters.append(f)
    everything = Gtk.FileFilter(name="All files")
    everything.add_pattern("*")
    filters.append(everything)
    return filters


class LibraryPage(Gtk.Box):
    def __init__(self, window):
        super().__init__(orientation=Gtk.Orientation.VERTICAL)
        self.window = window
        self.stack = Gtk.Stack(vexpand=True)
        self.append(self.stack)

        empty = Adw.StatusPage(
            title="No Miis yet",
            description="Miis you receive or import appear here.",
            icon_name="avatar-default-symbolic",
        )
        button = Gtk.Button(label="Import…", halign=Gtk.Align.CENTER)
        button.add_css_class("pill")
        button.add_css_class("suggested-action")
        button.connect("clicked", lambda _b: self.import_dialog())
        empty.set_child(button)
        self.stack.add_named(empty, "empty")

        page = Adw.PreferencesPage()
        self.group = Adw.PreferencesGroup(title="Saved Miis")
        add = Gtk.Button(icon_name="list-add-symbolic", tooltip_text="Import", valign=Gtk.Align.CENTER)
        add.add_css_class("flat")
        add.connect("clicked", lambda _b: self.import_dialog())
        self.group.set_header_suffix(add)
        page.add(self.group)
        self.stack.add_named(page, "list")
        self.rows = []

    @property
    def library(self):
        return Library(self.window.settings.library)

    def refresh(self):
        for row in self.rows:
            self.group.remove(row)
        self.rows = []
        entries = self.library.entries()
        for path, record in entries:
            row = Adw.ActionRow(title=GLib.markup_escape_text(record.name or "Unnamed"), activatable=True)
            if record.creator:
                row.set_subtitle(GLib.markup_escape_text(f"by {record.creator}"))
            if record.issues():
                row.add_suffix(Gtk.Image(icon_name="dialog-warning-symbolic", tooltip_text="A console would refuse it"))
            row.add_suffix(Gtk.Image(icon_name="go-next-symbolic"))
            row.connect("activated", lambda _r, p=path, rec=record: self.show(p, rec))
            self.group.add(row)
            self.rows.append(row)
        self.stack.set_visible_child_name("list" if entries else "empty")

    def show(self, path, record):
        dialog = DetailsDialog(record)
        if self.window.can_send():
            dialog.add_action("Send", lambda: (dialog.close(), self.window.send_record(record)), "suggested-action")
        dialog.add_action("Export…", lambda: self.export_dialog(record))
        dialog.add_action("Delete", lambda: self.confirm_delete(dialog, path, record), "destructive-action")
        dialog.present(self.window)

    def import_dialog(self):
        chooser = Gtk.FileDialog(title="Import Miis", filters=cfsd_filter())

        def done(d, result):
            try:
                files = d.open_multiple_finish(result)
            except GLib.Error:
                return
            added, failed = 0, []
            keys = self.window.settings.keys_text()
            for i in range(files.get_n_items()):
                path = files.get_item(i).get_path()
                try:
                    self.library.import_file(path, keys)
                    added += 1
                except (OSError, ValueError) as e:
                    failed.append(f"{GLib.path_get_basename(path)}: {e}")
            self.refresh()
            if failed:
                self.window.toast(failed[0] if len(failed) == 1 else f"{len(failed)} files could not be read")
            elif added:
                self.window.toast(f"Imported {added} Mii" + ("s" if added > 1 else ""))

        chooser.open_multiple(self.window, None, done)

    def export_dialog(self, record):
        chooser = Gtk.FileDialog(title="Export Mii", initial_name=(record.name or "mii") + SUFFIX)

        def done(d, result):
            try:
                target = d.save_finish(result)
            except GLib.Error:
                return
            try:
                Library.export(record, target.get_path())
                self.window.toast(f"Exported {record.name}")
            except OSError as e:
                self.window.toast(str(e))

        chooser.save(self.window, None, done)

    def confirm_delete(self, dialog, path, record):
        alert = Adw.AlertDialog(heading=f"Delete {record.name}?", body="The file is removed from the library folder.")
        alert.add_response("cancel", "Cancel")
        alert.add_response("delete", "Delete")
        alert.set_response_appearance("delete", Adw.ResponseAppearance.DESTRUCTIVE)

        def chosen(_a, response):
            if response == "delete":
                Library.delete(path)
                dialog.close()
                self.refresh()

        alert.connect("response", chosen)
        alert.present(self.window)
