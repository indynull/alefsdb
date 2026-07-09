# Supported FUSE edit paths

alefsdb mounts a **projection** of the typed namespace. Not every POSIX editor workflow is supported.

## Always supported

| Action | Result |
| --- | --- |
| `ls` directory | Namespace dirs + structure projections |
| `cat` scalar file | Text encoding of Null/Bool/Int/Float/String; raw for Bytes |
| `echo x > scalar` + flush | Type-stable replace of scalar (same scalar variant) |
| `mkdir` | Creates a namespace directory (parent must exist) |
| `rm` empty dir / value | Deletes namespace entry |
| `getfattr -n user.alefs.type` | Type metadata |

## Structure projections

| Type | Presentation | Writes |
| --- | --- | --- |
| hash | Directory of keys | Create file = insert string key → empty string; write scalar children type-stable; unlink removes key |
| list | `0`, `1`, … files/dirs | Unlink removes index; create not recommended |
| set | Hash-named member files | Unlink removes member |
| tree | Key-named entries | Unlink removes key |

## Not supported (expect `EPERM` / `ENOTSUP` / errors)

- Changing a value’s type via FUSE (e.g. string file → binary type)
- Full POSIX rename/reorder of list indices via arbitrary editor temp files
- Devices, symlinks, executables, permission models beyond simple modes

Use the CLI (`alefsdb set`, `hset`, `query`, …) for typed mutations when the shell mapping is ambiguous.
