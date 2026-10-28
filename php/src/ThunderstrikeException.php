<?php

declare(strict_types=1);

namespace Thunderstrike;

use FFI;
use RuntimeException;

/** Any non-OK status from the client or the server. */
class ThunderstrikeException extends RuntimeException
{
    private readonly string $detail;

    public function __construct(
        private readonly int $thunderstrikeCode,
        private readonly ?string $operation = null,
    ) {
        $text = self::describe($thunderstrikeCode);
        $where = $operation === null ? '' : $operation . ': ';
        // Read immediately: it is valid only until the next failing
        // call on this thread.
        $this->detail = self::lastErrorDetail();
        $tail = $this->detail === '' ? '' : ': ' . $this->detail;
        parent::__construct(
            "thunderstrike: {$where}{$text} (code {$thunderstrikeCode}){$tail}",
            $thunderstrikeCode,
        );
    }

    /**
     * The specific cause, when the native side recorded one.
     *
     * {@see describe} names the category ("TLS failed"); this names the
     * cause ("X509(NameMismatch)"). For TLS that difference is the
     * whole diagnosis - a certificate issued for another name, an
     * untrusted issuer and an expired one share a code and have
     * different fixes.
     */
    public function detail(): string
    {
        return $this->detail;
    }

    /** @internal */
    private static function lastErrorDetail(): string
    {
        try {
            return Lib::str(Lib::get()->thunderstrike_last_error_message());
        } catch (\Throwable) {
            // Formatting an error must never fail for a second reason.
            return '';
        }
    }

    /** The numeric status. See {@see Code}. */
    public function code(): int
    {
        return $this->thunderstrikeCode;
    }

    /**
     * Whether repeating the request is sound: Code::RETRY_ALWAYS (Busy,
     * not applied), Code::RETRY_IF_IDEMPOTENT (the outcome is unknown:
     * get/set/exists/ping only) or Code::RETRY_NEVER.
     */
    public function retryClass(): int
    {
        return self::retryClassOf($this->thunderstrikeCode);
    }

    /** The server answered Busy: not applied, retriable. */
    public function isBusy(): bool
    {
        return $this->thunderstrikeCode === Code::BUSY;
    }

    /** retryClass() for a bare code. */
    public static function retryClassOf(int $code): int
    {
        try {
            return (int) Lib::get()->thunderstrike_retry_class($code);
        } catch (\Throwable) {
            return Code::RETRY_NEVER;
        }
    }

    /** The operation that failed, when known. */
    public function operation(): ?string
    {
        return $this->operation;
    }

    /** The native library's description of a status code. */
    public static function describe(int $code): string
    {
        try {
            $text = Lib::str(Lib::get()->thunderstrike_strerror($code));

            return $text === '' ? 'unknown error' : $text;
        } catch (\Throwable) {
            // Formatting an error must never fail for a second reason.
            return 'unknown error';
        }
    }

    /** Throw unless $code is OK. */
    public static function check(int $code, string $operation): void
    {
        if ($code !== Code::OK) {
            throw self::of($code, $operation);
        }
    }

    public static function of(int $code, ?string $operation = null): self
    {
        return match ($code) {
            Code::NOT_FOUND    => new KeyNotFoundException($code, $operation),
            Code::TIMEOUT      => new TimeoutException($code, $operation),
            Code::DISCONNECTED => new DisconnectedException($code, $operation),
            Code::CLOSED       => new ClosedException($code, $operation),
            Code::BUSY         => new BusyException($code, $operation),
            default            => new self($code, $operation),
        };
    }
}
