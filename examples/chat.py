# Clients supply a unique request_id to make message retries safe.
# TODO(audit): support configurable page sizes and validate room/sender/message limits.
# TODO(audit): reject request_id reuse with changed room/sender/text instead of returning the original.
# TODO(chat): add monotonic acknowledgements and leased claims with fencing.
def post(room, sender, text, request_id):
    db_execute("app", "INSERT INTO messages(room, sender, text, request_id) VALUES (?, ?, ?, ?) ON CONFLICT(request_id) DO NOTHING", [room, sender, text, request_id])
    return db_query("app", "SELECT id, room, sender, text FROM messages WHERE request_id = ?", [request_id])["rows"][0]


def poll(room, after=0):
    return db_query("app", "SELECT id, room, sender, text FROM messages WHERE room = ? AND id > ? ORDER BY id LIMIT 100", [room, after])["rows"]
