"""The 'add a contact' web form."""


def handle_form_post(store, form):
    """Save whatever the person typed into the contact form.

    ``form`` is the parsed request body: a mapping of field name to string.
    """
    contact = {
        "name": form.get("name", ""),
        "email": form.get("email", ""),
    }
    if form.get("phone"):
        contact["phone"] = form["phone"]
    return store.save(contact)
