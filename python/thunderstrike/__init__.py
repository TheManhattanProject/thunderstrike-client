"""
thunderstrike_client — Python binding for the thunderstrike key-value database.

Pure ``ctypes``: no build step, no compiler, no extension module. It
loads the shared library that ships inside the package (or the one
``THUNDERSTRIKE_CLIENT_LIB`` names).

Usage::

    from thunderstrike import Thunderstrike

    with Thunderstrike("127.0.0.1:7379") as db:
        db.set(b"user:1", b"ada")
        print(db.get(b"user:1"))          # b'ada'
        print(db.get(b"nope"))            # None

        # One round trip, not 1000. Use this whenever you have more
        # than one key — it is the difference between a syscall per
        # key and a syscall per batch.
        values = db.mget([f"k:{i}".encode() for i in range(1000)])

Threading
---------
``Thunderstrike`` is safe to share across Python threads. ``ctypes.CDLL``
releases the GIL for the duration of each call, so a thread waiting on
a thunderstrike round trip does not block the rest of the interpreter — which
makes the blocking API a good fit for a thread pool.
"""

from __future__ import annotations

import ctypes
import os
import platform
import sys
from ctypes import (
    POINTER,
    Structure,
    byref,
    c_char_p,
    c_int32,
    c_size_t,
    c_uint8,
    c_uint16,
    c_uint32,
    c_uint64,
    c_void_p,
)
from typing import Iterable, Sequence

__all__ = [
    "Thunderstrike",
    "ThunderstrikeError",
    "NotFound",
    "Timeout",
    "Disconnected",
    "Closed",
    "Busy",
    "ServerOutOfMemory",
    "StorageExhausted",
    "ServerError",
    "TlsError",
    "Stats",
    "load_library",
    "retry_class",
]

# ---------------------------------------------------------------------------
# Status codes — mirrors include/thunderstrike_client.h
# ---------------------------------------------------------------------------

OK = 0
# Statuses the SERVER reported: the wire status byte.
NOT_FOUND = 1
WRONG_SHARD = 2
CAS_CONFLICT = 3          # never sent by the current server
SERVER_OOM = 4            # memory genuinely exhausted; not applied
NOT_AUTHED = 5            # never sent: the server closes instead
UNKNOWN_OP = 6
MALFORMED = 7             # e.g. an empty value; not applied
UNSUPPORTED_VERSION = 9
RESPONSE_TOO_LARGE = 11
STORAGE_EXHAUSTED = 12    # disk full or a structural ceiling; not applied
BUSY = 13                 # not applied; the one retriable status
SERVER_ERROR = 14         # integrity/device/invariant failure
UNKNOWN_STATUS = 64

ERR_INVALID_ARG = -1
ERR_CONFIG = -2
ERR_CONNECT = -3
ERR_IO = -4
ERR_TIMEOUT = -5
ERR_DISCONNECTED = -6
ERR_PROTOCOL = -7
ERR_KEY_TOO_LONG = -8
ERR_CLOSED = -9
ERR_BUFFER_TOO_SMALL = -10
ERR_RUNTIME = -11
ERR_PANIC = -12
ERR_TLS = -13             # was 13 before the server took that byte for Busy
ERR_VALUE_TOO_LARGE = -14

# retry_class(code)
RETRY_NEVER = 0
RETRY_IF_IDEMPOTENT = 1   # outcome unknown: repeat GET/SET/EXISTS/PING only
RETRY_ALWAYS = 2          # Busy: not applied, repeat anything

ROUTING_SHARD_BY_KEY = 0
ROUTING_ROUND_ROBIN = 1

MAX_KEY_LEN = 65535


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class ThunderstrikeError(Exception):
    """Any non-OK status. ``code`` is the numeric status.

    ``retry_class`` says whether repeating the request is sound:
    ``RETRY_ALWAYS`` (Busy: the server did not apply it),
    ``RETRY_IF_IDEMPOTENT`` (the outcome is unknown: GET/SET/EXISTS/PING
    only) or ``RETRY_NEVER``. The client has already retried what it
    safely could before raising.
    """

    def __init__(self, code: int, message: str = "", context: str = ""):
        self.code = code
        self.retry_class = retry_class(code)
        self.message = message or _lib_strerror(code)
        #: The specific cause, when the native side recorded one.
        #: `message` names the category ("TLS failed"); this names the
        #: cause ("X509(NameMismatch)"). For TLS that difference is the
        #: whole diagnosis - a certificate for another name, an
        #: untrusted issuer and an expired one share a code and have
        #: different fixes.
        self.detail = _lib_last_error()
        where = f"{context}: " if context else ""
        tail = f": {self.detail}" if self.detail else ""
        super().__init__(f"{where}{self.message} (code {code}){tail}")


class NotFound(ThunderstrikeError):
    """The key is not there. Only raised by APIs that promise a value."""


class Timeout(ThunderstrikeError):
    """The deadline elapsed. The server may still apply the request."""


class Disconnected(ThunderstrikeError):
    """The connection dropped mid-flight; the outcome is unknown."""


class Closed(ThunderstrikeError):
    """The client has been closed."""


class Busy(ThunderstrikeError):
    """The server was momentarily out of queue and did NOT apply the
    request. Retriable for any operation after a short backoff; the
    client already retried it (every operation but delete)."""

    @property
    def retriable(self) -> bool:
        return True


class ServerOutOfMemory(ThunderstrikeError):
    """The server's memory is genuinely exhausted. Not applied."""


class StorageExhausted(ThunderstrikeError):
    """The shard's disk is full, or it hit a structural ceiling. Not applied."""


class ServerError(ThunderstrikeError):
    """The server failed the operation (integrity, device, invariant)."""


class TlsError(ThunderstrikeError):
    """The TLS handshake failed or a record did not decrypt."""


_EXCEPTIONS = {
    NOT_FOUND: NotFound,
    ERR_TIMEOUT: Timeout,
    ERR_DISCONNECTED: Disconnected,
    ERR_CLOSED: Closed,
    BUSY: Busy,
    SERVER_OOM: ServerOutOfMemory,
    STORAGE_EXHAUSTED: StorageExhausted,
    SERVER_ERROR: ServerError,
    ERR_TLS: TlsError,
}


def retry_class(code: int) -> int:
    """``RETRY_NEVER`` / ``RETRY_IF_IDEMPOTENT`` / ``RETRY_ALWAYS`` for a code."""
    try:
        return int(load_library().thunderstrike_retry_class(code))
    except Exception:  # noqa: BLE001 - never let error formatting raise
        return RETRY_NEVER


def _raise(code: int, context: str = "") -> None:
    raise _EXCEPTIONS.get(code, ThunderstrikeError)(code, context=context)


# ---------------------------------------------------------------------------
# ctypes declarations
# ---------------------------------------------------------------------------


class _Buf(Structure):
    _fields_ = [
        ("data", POINTER(c_uint8)),
        ("len", c_size_t),
        ("owner", c_void_p),
    ]


class _Stats(Structure):
    _fields_ = [
        ("connections", c_uint64),
        ("connections_ready", c_uint64),
        ("inflight", c_uint64),
        ("capacity", c_uint64),
        ("requests", c_uint64),
        ("responses", c_uint64),
        ("bytes_written", c_uint64),
        ("bytes_read", c_uint64),
        ("write_batches", c_uint64),
        ("reconnects", c_uint64),
        ("orphan_responses", c_uint64),
        # The I/O pool. These must be here even if unused: the native
        # side writes the whole struct, so a short one is an overflow.
        ("io_threads", c_uint64),
        ("io_threads_min", c_uint64),
        ("io_threads_max", c_uint64),
        ("io_grew", c_uint64),
        ("io_shrank", c_uint64),
        ("io_moved", c_uint64),
    ]


_U8P = POINTER(c_uint8)
_U8PP = POINTER(_U8P)
_SIZEP = POINTER(c_size_t)

_lib = None


def _library_names() -> list[str]:
    system = platform.system()
    if system == "Windows":
        return ["thunderstrike_client.dll"]
    if system == "Darwin":
        return ["libthunderstrike_client.dylib"]
    return ["libthunderstrike_client.so"]


def _search_paths() -> Iterable[str]:
    """Where to look, most specific first."""
    override = os.environ.get("THUNDERSTRIKE_CLIENT_LIB")
    if override:
        yield override
    here = os.path.dirname(os.path.abspath(__file__))
    roots = [
        here,
        # bindings/python/thunderstrike -> bindings/python -> bindings -> repo root
        os.path.join(here, "..", "..", "..", "target", "release"),
        os.path.join(here, "..", "..", "..", "target", "debug"),
    ]
    for root in roots:
        for name in _library_names():
            yield os.path.join(root, name)
    # Finally, let the loader search the system paths.
    for name in _library_names():
        yield name


def load_library(path: str | None = None) -> ctypes.CDLL:
    """Load (once) and return the shared library.

    Set ``THUNDERSTRIKE_CLIENT_LIB`` to point at a specific build.
    """
    global _lib
    if _lib is not None and path is None:
        return _lib

    candidates = [path] if path else list(_search_paths())
    errors = []
    lib = None
    for candidate in candidates:
        try:
            # CDLL (not PyDLL): releases the GIL around each call.
            lib = ctypes.CDLL(candidate)
            break
        except OSError as e:  # noqa: PERF203 - the loop *is* the fallback
            errors.append(f"  {candidate}: {e}")
    if lib is None:
        raise OSError(
            "could not load the thunderstrike client library. Install the\n"
            "wheel for this platform, or set THUNDERSTRIKE_CLIENT_LIB to the\n"
            "library's path. Tried:\n" + "\n".join(errors)
        )

    _declare(lib)
    if path is None:
        _lib = lib
    return lib


def _declare(lib: ctypes.CDLL) -> None:
    """Attach argtypes/restype to every symbol we call.

    Not optional: without argtypes, ctypes guesses, and on 64-bit
    Windows a guessed pointer is truncated to 32 bits.
    """
    sig = [
        ("thunderstrike_config_new", [], c_void_p),
        ("thunderstrike_config_free", [c_void_p], None),
        ("thunderstrike_config_add_endpoint", [c_void_p, c_char_p], c_int32),
        ("thunderstrike_config_set_cluster", [c_void_p, c_char_p, c_uint16, c_uint16, c_uint16], c_int32),
        ("thunderstrike_config_set_connections_per_endpoint", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_max_inflight", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_max_buffered_bytes", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_max_response_bytes", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_request_timeout_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_connect_timeout_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_tcp_keepalive_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_tcp_nodelay", [c_void_p, c_int32], c_int32),
        ("thunderstrike_config_set_max_retries", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_busy_retries", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_busy_backoff_ms", [c_void_p, c_uint64, c_uint64], c_int32),
        ("thunderstrike_config_set_busy_retry_window_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_timeout_per_mib_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_insecure_plaintext", [c_void_p, c_int32], c_int32),
        ("thunderstrike_config_set_multiplexing", [c_void_p, c_int32], c_int32),
        ("thunderstrike_config_set_mux_chunk_bytes", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_mux_max_open_bodies", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_get_mux_stats", [c_void_p, POINTER(c_uint64), POINTER(c_uint64), POINTER(c_uint64)], c_int32),
        ("thunderstrike_config_set_client_identity", [c_void_p, POINTER(c_uint8), c_size_t, POINTER(c_uint8), c_size_t], c_int32),
        ("thunderstrike_retry_class", [c_int32], c_int32),
        ("thunderstrike_config_set_routing", [c_void_p, c_int32], c_int32),
        ("thunderstrike_config_set_worker_threads", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_last_error_message", [], c_char_p),
        ("thunderstrike_config_set_tls", [c_void_p, c_char_p, POINTER(c_uint8), c_size_t], c_int32),
        ("thunderstrike_config_set_min_io_threads", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_max_io_threads", [c_void_p, c_uint32], c_int32),
        ("thunderstrike_config_set_io_sample_interval_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_config_set_io_idle_timeout_ms", [c_void_p, c_uint64], c_int32),
        ("thunderstrike_connect", [c_void_p, POINTER(c_void_p)], c_int32),
        ("thunderstrike_client_free", [c_void_p], None),
        ("thunderstrike_get", [c_void_p, _U8P, c_size_t, POINTER(_Buf)], c_int32),
        ("thunderstrike_get_into", [c_void_p, _U8P, c_size_t, _U8P, c_size_t, _SIZEP], c_int32),
        ("thunderstrike_set", [c_void_p, _U8P, c_size_t, _U8P, c_size_t], c_int32),
        ("thunderstrike_set_ttl", [c_void_p, _U8P, c_size_t, _U8P, c_size_t, c_uint32], c_int32),
        ("thunderstrike_delete", [c_void_p, _U8P, c_size_t], c_int32),
        ("thunderstrike_exists", [c_void_p, _U8P, c_size_t], c_int32),
        ("thunderstrike_append", [c_void_p, _U8P, c_size_t, _U8P, c_size_t, POINTER(c_uint64)], c_int32),
        ("thunderstrike_ping", [c_void_p], c_int32),
        ("thunderstrike_mget", [c_void_p, _U8PP, _SIZEP, c_size_t, POINTER(_Buf), POINTER(c_int32)], c_int32),
        ("thunderstrike_mset", [c_void_p, _U8PP, _SIZEP, _U8PP, _SIZEP, c_size_t, POINTER(c_int32)], c_int32),
        ("thunderstrike_mdelete", [c_void_p, _U8PP, _SIZEP, c_size_t, POINTER(c_int32)], c_int32),
        ("thunderstrike_buf_free", [POINTER(_Buf)], None),
        ("thunderstrike_get_stats", [c_void_p, POINTER(_Stats)], c_int32),
        ("thunderstrike_shard_of", [c_void_p, _U8P, c_size_t, POINTER(c_uint16)], c_int32),
        ("thunderstrike_hash_shard", [_U8P, c_size_t, c_uint16, POINTER(c_uint16)], c_int32),
        ("thunderstrike_shard_count", [c_void_p, POINTER(c_uint16)], c_int32),
        ("thunderstrike_endpoint_count", [c_void_p, POINTER(c_uint32)], c_int32),
        ("thunderstrike_strerror", [c_int32], c_char_p),
        ("thunderstrike_version", [], c_char_p),
    ]
    for name, argtypes, restype in sig:
        fn = getattr(lib, name)
        fn.argtypes = argtypes
        fn.restype = restype


def _lib_strerror(code: int) -> str:
    try:
        return load_library().thunderstrike_strerror(code).decode("utf-8", "replace")
    except Exception:  # noqa: BLE001 - never let error formatting raise
        return "unknown error"


def _lib_last_error() -> str:
    """The detail behind this thread's most recent failure, or "".

    Valid only until the next failing call on this thread, which is why
    it is read immediately when an exception is constructed.
    """
    try:
        p = load_library().thunderstrike_last_error_message()
        return p.decode("utf-8", "replace") if p else ""
    except Exception:  # noqa: BLE001 - never let error formatting raise
        return ""


def _as_u8p(b: bytes):
    """Borrow a ``bytes`` as ``const uint8_t *``.

    The returned pointer is only valid while `b` is alive — every use
    below keeps the original object referenced for the whole call.
    """
    return ctypes.cast(ctypes.c_char_p(b), _U8P)


def version() -> str:
    return load_library().thunderstrike_version().decode()


def shard_of(key: bytes, shards: int) -> int:
    """Which of ``shards`` shards owns ``key``: ``CRC32C(key) % shards``.

    The same function the server places data with, so a caller routing
    for itself lands where the server expects.
    """
    lib = load_library()
    out = c_uint16()
    rc = lib.thunderstrike_hash_shard(_as_u8p(key), len(key), shards, byref(out))
    if rc != OK:
        _raise(rc, "shard_of")
    return out.value


# ---------------------------------------------------------------------------
# Stats
# ---------------------------------------------------------------------------


class Stats:
    """Pool counters."""

    __slots__ = [f[0] for f in _Stats._fields_]

    def __init__(self, raw: _Stats):
        for name, _ in _Stats._fields_:
            setattr(self, name, getattr(raw, name))

    @property
    def batching_factor(self) -> float:
        """Requests per ``write`` syscall. Higher is better."""
        return self.requests / self.write_batches if self.write_batches else 0.0

    def __repr__(self) -> str:
        return (
            f"Stats(connections={self.connections}/{self.connections_ready} ready, "
            f"requests={self.requests}, writes={self.write_batches}, "
            f"batch={self.batching_factor:.1f}x, inflight={self.inflight}, "
            f"reconnects={self.reconnects})"
        )


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------


class Thunderstrike:
    """A pooled, pipelined thunderstrike client.

    Safe to share across threads. Close it with ``close()`` or by using
    it as a context manager.
    """

    def __init__(
        self,
        endpoints: str | Sequence[str] | None = None,
        *,
        host: str | None = None,
        base_port: int | None = None,
        shards: int | None = None,
        cores: int | None = None,
        connections_per_endpoint: int | None = None,
        max_inflight: int | None = None,
        max_buffered_bytes: int | None = None,
        max_response_bytes: int | None = None,
        # Default 60000: longer than the server's own bounded waits, so
        # its genuine Busy/Oom arrives before the client gives up.
        request_timeout_ms: int | None = None,
        connect_timeout_ms: int | None = None,
        tcp_keepalive_ms: int | None = None,
        tcp_nodelay: bool | None = None,
        max_retries: int | None = None,
        # Busy (not applied) is retried with a jittered backoff, for
        # every operation but delete. Defaults 4 / (2, 200) ms / 5000.
        busy_retries: int | None = None,
        busy_backoff_ms: tuple[int, int] | None = None,
        busy_retry_window_ms: int | None = None,
        timeout_per_mib_ms: int | None = None,
        routing: int | None = None,
        worker_threads: int | None = None,
        # TLS 1.3, which the thunderstrike server REQUIRES. Both are needed
        # together.
        #
        # `tls_server_name` is sent as SNI AND matched against the
        # certificate's subject alternative names, so it must be a name
        # the certificate was issued for -- not necessarily the host in
        # the endpoint. `tls_ca_pem` is PEM text: the CA certificate
        # (ca.crt) that issued the server's. No way to skip verification.
        tls_server_name: str | None = None,
        tls_ca_pem: bytes | str | None = None,
        # mTLS: the client certificate chain (leaf first) and its PKCS#8
        # Ed25519 key, both PEM, for a server started with
        # THUNDERSTRIKE_TLS_CLIENT_CA.
        tls_client_cert_pem: bytes | str | None = None,
        tls_client_key_pem: bytes | str | None = None,
        # Connect WITHOUT TLS. Only the in-repo mock server answers
        # that; the thunderstrike server is TLS-only.
        insecure_plaintext: bool = False,
        # The I/O pool. Connections are sockets; their readers and
        # writers are tasks, so 1024 connections are not 1024 threads.
        # These decide how many cores the client may occupy.
        min_io_threads: int | None = None,
        max_io_threads: int | None = None,
        io_sample_interval_ms: int | None = None,
        io_idle_timeout_ms: int | None = None,
        # Frame multiplexing, on by default: a large value travels in
        # chunks with other requests and replies between them, so a
        # small op never waits behind a whole large one on its
        # connection. False forces the legacy framing (no HELLO).
        multiplexing: bool | None = None,
        # Chunk size for large values on a multiplexed connection
        # (default 1 MiB, held to the server's grant).
        mux_chunk_bytes: int | None = None,
        # Chunked request bodies open at once on a multiplexed connection
        # (default 1, held to the server's grant; more than one is unsafe
        # against the server at 36db4921 under arena pressure).
        mux_max_open_bodies: int | None = None,
        library: str | None = None,
    ):
        """Connect.

        ``endpoints`` is a bootstrap address, or a list of them. ONE
        is enough: the client asks it for the deployment's routing
        table and opens a connection to every port that names.

        ``host``/``base_port``/``shards``/``cores`` instead *state*
        the layout of a server started with ``--shards n --cores m``
        — core *c* listens on ``base_port + c``. Optional, and usually
        unnecessary; what it adds is a check, because the stated
        layout is compared against the one the server serves and the
        connect fails if they differ.
        """
        self._lib = load_library(library)
        self._handle = None

        cfg = self._lib.thunderstrike_config_new()
        if not cfg:
            raise MemoryError("thunderstrike_config_new failed")
        try:
            if isinstance(endpoints, str):
                endpoints = [endpoints]
            for addr in endpoints or []:
                self._check(self._lib.thunderstrike_config_add_endpoint(cfg, addr.encode()), "add_endpoint")
            if host is not None:
                if base_port is None or shards is None or cores is None:
                    raise ValueError("host needs base_port, shards and cores")
                self._check(
                    self._lib.thunderstrike_config_set_cluster(
                        cfg, host.encode(), base_port, shards, cores
                    ),
                    "set_cluster",
                )

            for value, fn, name in (
                (connections_per_endpoint, "thunderstrike_config_set_connections_per_endpoint", "connections_per_endpoint"),
                (max_inflight, "thunderstrike_config_set_max_inflight", "max_inflight"),
                (max_buffered_bytes, "thunderstrike_config_set_max_buffered_bytes", "max_buffered_bytes"),
                (max_response_bytes, "thunderstrike_config_set_max_response_bytes", "max_response_bytes"),
                (request_timeout_ms, "thunderstrike_config_set_request_timeout_ms", "request_timeout_ms"),
                (connect_timeout_ms, "thunderstrike_config_set_connect_timeout_ms", "connect_timeout_ms"),
                (tcp_keepalive_ms, "thunderstrike_config_set_tcp_keepalive_ms", "tcp_keepalive_ms"),
                (max_retries, "thunderstrike_config_set_max_retries", "max_retries"),
                (busy_retries, "thunderstrike_config_set_busy_retries", "busy_retries"),
                (busy_retry_window_ms, "thunderstrike_config_set_busy_retry_window_ms", "busy_retry_window_ms"),
                (timeout_per_mib_ms, "thunderstrike_config_set_timeout_per_mib_ms", "timeout_per_mib_ms"),
                (routing, "thunderstrike_config_set_routing", "routing"),
                (worker_threads, "thunderstrike_config_set_worker_threads", "worker_threads"),
                (min_io_threads, "thunderstrike_config_set_min_io_threads", "min_io_threads"),
                (max_io_threads, "thunderstrike_config_set_max_io_threads", "max_io_threads"),
                (io_sample_interval_ms, "thunderstrike_config_set_io_sample_interval_ms", "io_sample_interval_ms"),
                (io_idle_timeout_ms, "thunderstrike_config_set_io_idle_timeout_ms", "io_idle_timeout_ms"),
                (mux_chunk_bytes, "thunderstrike_config_set_mux_chunk_bytes", "mux_chunk_bytes"),
                (mux_max_open_bodies, "thunderstrike_config_set_mux_max_open_bodies", "mux_max_open_bodies"),
            ):
                if value is not None:
                    self._check(getattr(self._lib, fn)(cfg, value), name)
            if tls_server_name is not None or tls_ca_pem is not None:
                if tls_server_name is None or tls_ca_pem is None:
                    raise ValueError(
                        "tls_server_name and tls_ca_pem must be given together - "
                        "a trust anchor with no name to check it against "
                        "verifies nothing"
                    )
                pem = (
                    tls_ca_pem.encode()
                    if isinstance(tls_ca_pem, str)
                    else tls_ca_pem
                )
                buf = (c_uint8 * len(pem)).from_buffer_copy(pem)
                self._check(
                    self._lib.thunderstrike_config_set_tls(
                        cfg,
                        tls_server_name.encode(),
                        ctypes.cast(buf, POINTER(c_uint8)),
                        len(pem),
                    ),
                    "tls",
                )
            if busy_backoff_ms is not None:
                lo, hi = busy_backoff_ms
                self._check(self._lib.thunderstrike_config_set_busy_backoff_ms(cfg, lo, hi), "busy_backoff_ms")
            if (tls_client_cert_pem is None) != (tls_client_key_pem is None):
                raise ValueError("tls_client_cert_pem and tls_client_key_pem go together")
            if tls_client_cert_pem is not None:
                cert = tls_client_cert_pem.encode() if isinstance(tls_client_cert_pem, str) else tls_client_cert_pem
                key = tls_client_key_pem.encode() if isinstance(tls_client_key_pem, str) else tls_client_key_pem
                cbuf = (c_uint8 * len(cert)).from_buffer_copy(cert)
                kbuf = (c_uint8 * len(key)).from_buffer_copy(key)
                self._check(
                    self._lib.thunderstrike_config_set_client_identity(
                        cfg,
                        ctypes.cast(cbuf, POINTER(c_uint8)),
                        len(cert),
                        ctypes.cast(kbuf, POINTER(c_uint8)),
                        len(key),
                    ),
                    "client_identity",
                )
            if insecure_plaintext:
                self._check(self._lib.thunderstrike_config_set_insecure_plaintext(cfg, 1), "insecure_plaintext")
            if tcp_nodelay is not None:
                self._check(self._lib.thunderstrike_config_set_tcp_nodelay(cfg, 1 if tcp_nodelay else 0), "tcp_nodelay")
            if multiplexing is not None:
                self._check(self._lib.thunderstrike_config_set_multiplexing(cfg, 1 if multiplexing else 0), "multiplexing")

            handle = c_void_p()
            self._check(self._lib.thunderstrike_connect(cfg, byref(handle)), "connect")
            self._handle = handle
        finally:
            self._lib.thunderstrike_config_free(cfg)

    # -- lifecycle ---------------------------------------------------

    def close(self) -> None:
        """Shut the pool down. Idempotent."""
        if self._handle is not None:
            self._lib.thunderstrike_client_free(self._handle)
            self._handle = None

    def __enter__(self) -> "Thunderstrike":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:  # noqa: BLE001 - never raise from __del__
            pass

    def _h(self) -> c_void_p:
        if self._handle is None:
            raise Closed(ERR_CLOSED)
        return self._handle

    @staticmethod
    def _check(rc: int, context: str) -> None:
        if rc != OK:
            _raise(rc, context)

    # -- single keys -------------------------------------------------

    def get(self, key: bytes, default=None):
        """Return the value, or ``default`` when the key is absent."""
        buf = _Buf()
        rc = self._lib.thunderstrike_get(self._h(), _as_u8p(key), len(key), byref(buf))
        try:
            if rc == NOT_FOUND:
                return default
            if rc != OK:
                _raise(rc, "get")
            if buf.len == 0:
                return b""
            return ctypes.string_at(buf.data, buf.len)
        finally:
            # Frees the client's hold on the read buffer; safe even
            # when nothing was written into `buf`.
            self._lib.thunderstrike_buf_free(byref(buf))

    def __getitem__(self, key: bytes) -> bytes:
        v = self.get(key, _MISSING)
        if v is _MISSING:
            raise NotFound(NOT_FOUND, context="get")
        return v

    def set(self, key: bytes, value: bytes, ttl_ms: int = 0) -> None:
        rc = self._lib.thunderstrike_set_ttl(
            self._h(), _as_u8p(key), len(key), _as_u8p(value), len(value), ttl_ms
        )
        self._check(rc, "set")

    def __setitem__(self, key: bytes, value: bytes) -> None:
        self.set(key, value)

    def delete(self, key: bytes) -> bool:
        """``True`` if the key was there."""
        rc = self._lib.thunderstrike_delete(self._h(), _as_u8p(key), len(key))
        if rc == OK:
            return True
        if rc == NOT_FOUND:
            return False
        _raise(rc, "delete")

    def __delitem__(self, key: bytes) -> None:
        if not self.delete(key):
            raise NotFound(NOT_FOUND, context="delete")

    def exists(self, key: bytes) -> bool:
        rc = self._lib.thunderstrike_exists(self._h(), _as_u8p(key), len(key))
        if rc == OK:
            return True
        if rc == NOT_FOUND:
            return False
        _raise(rc, "exists")

    def __contains__(self, key: bytes) -> bool:
        return self.exists(key)

    def append(self, key: bytes, value: bytes) -> int:
        """Append to a value, returning its new total length."""
        total = c_uint64()
        rc = self._lib.thunderstrike_append(
            self._h(), _as_u8p(key), len(key), _as_u8p(value), len(value), byref(total)
        )
        self._check(rc, "append")
        return total.value

    def ping(self) -> None:
        self._check(self._lib.thunderstrike_ping(self._h()), "ping")

    # -- batches -----------------------------------------------------

    def mget(self, keys: Sequence[bytes]) -> list[bytes | None]:
        """Read many keys in ONE round trip. ``None`` for a miss.

        Prefer this to a loop over ``get`` for anything above a couple
        of keys: the whole batch goes on the wire before any answer is
        awaited.
        """
        n = len(keys)
        if n == 0:
            return []
        key_ptrs, key_lens, _keep = _pack(keys)
        bufs = (_Buf * n)()
        codes = (c_int32 * n)()

        rc = self._lib.thunderstrike_mget(self._h(), key_ptrs, key_lens, n, bufs, codes)
        try:
            self._check(rc, "mget")
            out: list[bytes | None] = []
            for i in range(n):
                c = codes[i]
                if c == OK:
                    out.append(ctypes.string_at(bufs[i].data, bufs[i].len) if bufs[i].len else b"")
                elif c == NOT_FOUND:
                    out.append(None)
                else:
                    _raise(c, f"mget[{i}]")
            return out
        finally:
            for i in range(n):
                self._lib.thunderstrike_buf_free(byref(bufs[i]))

    def mset(self, items) -> None:
        """Write many pairs in ONE round trip.

        ``items`` is a mapping or an iterable of ``(key, value)``.
        """
        pairs = list(items.items() if hasattr(items, "items") else items)
        n = len(pairs)
        if n == 0:
            return
        key_ptrs, key_lens, _keep_k = _pack([k for k, _ in pairs])
        val_ptrs, val_lens, _keep_v = _pack([v for _, v in pairs])
        codes = (c_int32 * n)()

        rc = self._lib.thunderstrike_mset(self._h(), key_ptrs, key_lens, val_ptrs, val_lens, n, codes)
        self._check(rc, "mset")
        for i in range(n):
            if codes[i] != OK:
                _raise(codes[i], f"mset[{i}]")

    def mdelete(self, keys: Sequence[bytes]) -> list[bool]:
        """Delete many keys in ONE round trip."""
        n = len(keys)
        if n == 0:
            return []
        key_ptrs, key_lens, _keep = _pack(keys)
        codes = (c_int32 * n)()
        rc = self._lib.thunderstrike_mdelete(self._h(), key_ptrs, key_lens, n, codes)
        self._check(rc, "mdelete")
        out = []
        for i in range(n):
            if codes[i] == OK:
                out.append(True)
            elif codes[i] == NOT_FOUND:
                out.append(False)
            else:
                _raise(codes[i], f"mdelete[{i}]")
        return out

    # -- introspection ----------------------------------------------

    def stats(self) -> Stats:
        raw = _Stats()
        self._check(self._lib.thunderstrike_get_stats(self._h(), byref(raw)), "stats")
        return Stats(raw)

    def mux_stats(self) -> dict:
        """Frame multiplexing counters: ``connections`` multiplexed and up
        now, ``chunked_requests`` sent in chunks, ``chunked_replies``
        received in chunks."""
        conns, req, rep = c_uint64(), c_uint64(), c_uint64()
        self._check(
            self._lib.thunderstrike_get_mux_stats(self._h(), byref(conns), byref(req), byref(rep)),
            "mux_stats",
        )
        return {"connections": conns.value, "chunked_requests": req.value, "chunked_replies": rep.value}

    def shard_of(self, key: bytes) -> int:
        """Which shard owns ``key``, under the table read from the server."""
        out = c_uint16()
        self._check(
            self._lib.thunderstrike_shard_of(self._h(), _as_u8p(key), len(key), byref(out)),
            "shard_of",
        )
        return out.value

    def shards(self) -> int:
        """How many shards the deployment has, as the server stated it."""
        out = c_uint16()
        self._check(self._lib.thunderstrike_shard_count(self._h(), byref(out)), "shard_count")
        return out.value

    def endpoints(self) -> int:
        """How many endpoints this client holds — one per server core."""
        out = c_uint32()
        self._check(self._lib.thunderstrike_endpoint_count(self._h(), byref(out)), "endpoint_count")
        return out.value


class _Missing:
    pass


_MISSING = _Missing()


def _pack(items: Sequence[bytes]):
    """Build the parallel pointer/length arrays the batch ABI takes.

    Returns ``(ptrs, lens, keep)``. ``keep`` owns every buffer the
    pointers refer to — a ``bytearray`` or ``memoryview`` argument is
    converted here, and without ``keep`` that temporary could be
    collected while the call is still reading it. Callers must hold
    ``keep`` until the call returns.
    """
    n = len(items)
    ptrs = (_U8P * n)()
    lens = (c_size_t * n)()
    keep: list[bytes] = []
    for i, item in enumerate(items):
        if isinstance(item, bytes):
            b = item
        elif isinstance(item, (bytearray, memoryview)):
            b = bytes(item)
        else:
            raise TypeError(f"item {i} is {type(item).__name__}, expected bytes")
        keep.append(b)
        ptrs[i] = _as_u8p(b)
        lens[i] = len(b)
    return ptrs, lens, keep


def options_from_env() -> dict:
    """TLS options from THUNDERSTRIKE_CA / THUNDERSTRIKE_SERVER_NAME (and
    THUNDERSTRIKE_CLIENT_CERT / THUNDERSTRIKE_CLIENT_KEY for mTLS), or plaintext for
    the mock server when THUNDERSTRIKE_CA is unset."""
    ca = os.environ.get("THUNDERSTRIKE_CA")
    if not ca:
        return {"insecure_plaintext": True}
    opts = {
        "tls_server_name": os.environ.get("THUNDERSTRIKE_SERVER_NAME", "localhost"),
        "tls_ca_pem": open(ca, "rb").read(),
    }
    cert, key = os.environ.get("THUNDERSTRIKE_CLIENT_CERT"), os.environ.get("THUNDERSTRIKE_CLIENT_KEY")
    if cert and key:
        opts["tls_client_cert_pem"] = open(cert, "rb").read()
        opts["tls_client_key_pem"] = open(key, "rb").read()
    return opts


if __name__ == "__main__":
    addr = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1:7379"
    with Thunderstrike(addr, **options_from_env()) as db:
        print("thunderstrike client", version(), "->", addr)
        db.ping()
        db.set(b"user:1", b"ada")
        print("user:1 =", db.get(b"user:1"))
        print("missing =", db.get(b"nope"))
        db.mset({f"k:{i}".encode(): f"v:{i}".encode() for i in range(1000)})
        got = db.mget([f"k:{i}".encode() for i in range(1000)])
        print("mget read", sum(1 for g in got if g is not None), "of 1000")
        print(db.stats())
