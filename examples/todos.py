def add(text):
    return db_execute("app", "INSERT INTO todos(text) VALUES (?) RETURNING id, text, done", [text])["rows"][0]


def complete(id):
    return db_execute("app", "UPDATE todos SET done = 1 WHERE id = ? RETURNING id, text, done", [id])["rows"]


def list_todos():
    return db_query("app", "SELECT id, text, done FROM todos ORDER BY id")["rows"]
