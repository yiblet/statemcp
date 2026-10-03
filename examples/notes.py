def add(text):
    return db_execute("app", "INSERT INTO notes(text) VALUES (?) RETURNING id, text", [text])["rows"][0]


def list_notes():
    return db_query("app", "SELECT id, text FROM notes ORDER BY id")["rows"]
