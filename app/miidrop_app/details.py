"""A dialog showing everything stored in one Mii."""

from gi.repository import Adw, Gtk

COLOURS = [
    "Red", "Orange", "Yellow", "Light green", "Green", "Blue",
    "Light blue", "Pink", "Purple", "Brown", "White", "Black",
]
MONTHS = [
    "January", "February", "March", "April", "May", "June", "July",
    "August", "September", "October", "November", "December",
]
GROUPS = [
    ("Face", ("face_", "skin_", "wrinkles", "makeup")),
    ("Hair", ("hair_",)),
    ("Eyes", ("eye_",)),
    ("Eyebrows", ("eyebrow_",)),
    ("Nose", ("nose_",)),
    ("Mouth", ("mouth_",)),
    ("Facial hair", ("moustache_", "beard_", "facial_hair_")),
    ("Glasses", ("glasses_",)),
    ("Mole", ("mole",)),
]
HIDDEN = {"region_lock", "char_set", "page", "slot", "origin", "gender", "favourite_colour", "height", "weight"}


def birthday_text(record):
    month, day = record.birthday
    if not month or not day or month > 12:
        return "Not set"
    return f"{day} {MONTHS[month - 1]}"


def label(name):
    return name.replace("_", " ").capitalize()


def info_row(title, value):
    row = Adw.ActionRow(title=title, subtitle=str(value), subtitle_selectable=True)
    row.add_css_class("property")
    return row


class DetailsDialog(Adw.Dialog):
    """Shows a record, with buttons the caller adds for what can be done with it."""

    def __init__(self, record, title=None):
        super().__init__(title=title or record.name or "Mii", content_width=460, content_height=640)
        self.record = record
        view = Adw.ToolbarView()
        view.add_top_bar(Adw.HeaderBar())
        page = Adw.PreferencesPage()
        view.set_content(page)
        self.actions = Gtk.Box(spacing=12, halign=Gtk.Align.CENTER, margin_top=12, margin_bottom=12)
        view.add_bottom_bar(self.actions)
        self.set_child(view)

        fields = record.fields()
        about = Adw.PreferencesGroup()
        about.add(info_row("Name", record.name or "—"))
        about.add(info_row("Creator", record.creator or "—"))
        about.add(info_row("Birthday", birthday_text(record)))
        about.add(info_row("Gender", "Female" if fields.get("gender") else "Male"))
        colour = fields.get("favourite_colour", 0)
        about.add(info_row("Favourite colour", COLOURS[colour] if colour < len(COLOURS) else colour))
        about.add(info_row("Height", fields.get("height")))
        about.add(info_row("Weight", fields.get("weight")))
        page.add(about)

        flags = Adw.PreferencesGroup(title="Sharing")
        flags.add(info_row("Others can copy it", "Yes" if record.copying_allowed else "No"))
        flags.add(info_row("Can be shared", "No" if record.sharing_disabled else "Yes"))
        page.add(flags)

        issues = record.issues()
        check = Adw.PreferencesGroup(title="Check")
        if issues:
            for issue in issues:
                row = Adw.ActionRow(title=issue)
                row.add_prefix(Gtk.Image(icon_name="dialog-warning-symbolic"))
                check.add(row)
        else:
            row = Adw.ActionRow(title="A console will accept this Mii")
            row.add_prefix(Gtk.Image(icon_name="emblem-ok-symbolic"))
            check.add(row)
        page.add(check)

        looks = Adw.PreferencesGroup(title="Appearance")
        for name, prefixes in GROUPS:
            expander = Adw.ExpanderRow(title=name)
            for key, value in fields.items():
                if key not in HIDDEN and key.startswith(prefixes):
                    expander.add_row(info_row(label(key), value))
            looks.add(expander)
        page.add(looks)

        ids = Adw.PreferencesGroup(title="Identity")
        ids.add(info_row("Mii ID", f"{record.mii_id:08x}"))
        ids.add(info_row("System ID", record.system_id))
        ids.add(info_row("Creator MAC", record.creator_mac))
        page.add(ids)

    def add_action(self, text, callback, style=None):
        button = Gtk.Button(label=text)
        button.add_css_class("pill")
        if style:
            button.add_css_class(style)
        button.connect("clicked", lambda _b: callback())
        self.actions.append(button)
        return button
