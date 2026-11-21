"""Mii pictures as avatars, falling back to initials while loading or offline."""

from gi.repository import Adw, Gdk, GLib

# Two widths keep the cache small: one for list rows, one for the details dialog.
SMALL, LARGE = 96, 384


def mii_avatar(record, size, renderer):
    avatar = Adw.Avatar(size=size, text=record.name or "?", show_initials=True)
    if renderer is None:
        return avatar
    width = SMALL if size <= 48 else LARGE
    data = bytes(record)

    def apply(png):
        if png:
            try:
                avatar.set_custom_image(Gdk.Texture.new_from_bytes(GLib.Bytes.new(png)))
            except GLib.Error:
                pass
        return GLib.SOURCE_REMOVE

    png = renderer.cached(data, width)
    if png:
        apply(png)
    else:
        renderer.request(data, width, lambda p: GLib.idle_add(apply, p))
    return avatar
