<?php

declare(strict_types=1);

namespace Thunderstrike;

use FFI;

/** Pool counters. */
final class Stats
{
    public function __construct(
        /** Connections in the pool. */
        public readonly int $connections,
        /** Connections currently usable. */
        public readonly int $connectionsReady,
        /** Requests on the wire right now. */
        public readonly int $inflight,
        /** Requests that could be on the wire at once. */
        public readonly int $capacity,
        public readonly int $requests,
        public readonly int $responses,
        public readonly int $bytesWritten,
        public readonly int $bytesRead,
        /** `write` syscalls. */
        public readonly int $writeBatches,
        public readonly int $reconnects,
        /** Answers whose waiter had already given up. */
        public readonly int $orphanResponses,
        /**
         * I/O threads running now — the cores this client can be
         * using. Connections are sockets; their readers and writers
         * are tasks, so the connection count is not this number.
         */
        public readonly int $ioThreads = 0,
        /** The floor the pool may narrow to. */
        public readonly int $ioThreadsMin = 0,
        /** The ceiling the pool may widen to. */
        public readonly int $ioThreadsMax = 0,
        /** Times the pool widened. */
        public readonly int $ioGrew = 0,
        /** Times the pool narrowed. */
        public readonly int $ioShrank = 0,
        /** Connections moved between threads to keep the split even. */
        public readonly int $ioMoved = 0,
    ) {
    }

    /** @internal */
    public static function from(FFI\CData $raw): self
    {
        return new self(
            (int) $raw->connections,
            (int) $raw->connections_ready,
            (int) $raw->inflight,
            (int) $raw->capacity,
            (int) $raw->requests,
            (int) $raw->responses,
            (int) $raw->bytes_written,
            (int) $raw->bytes_read,
            (int) $raw->write_batches,
            (int) $raw->reconnects,
            (int) $raw->orphan_responses,
            (int) $raw->io_threads,
            (int) $raw->io_threads_min,
            (int) $raw->io_threads_max,
            (int) $raw->io_grew,
            (int) $raw->io_shrank,
            (int) $raw->io_moved,
        );
    }

    /**
     * Requests per `write` syscall — the payoff from coalescing. 1.0
     * means no batching happened; higher is better.
     */
    public function batchingFactor(): float
    {
        return $this->writeBatches === 0 ? 0.0 : $this->requests / $this->writeBatches;
    }

    public function __toString(): string
    {
        return \sprintf(
            'connections=%d (%d ready), requests=%d, writes=%d (%.1fx), inflight=%d, reconnects=%d',
            $this->connections,
            $this->connectionsReady,
            $this->requests,
            $this->writeBatches,
            $this->batchingFactor(),
            $this->inflight,
            $this->reconnects,
        );
    }
}
