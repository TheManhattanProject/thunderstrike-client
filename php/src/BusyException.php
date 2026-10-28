<?php

declare(strict_types=1);

namespace Thunderstrike;

/**
 * The server was momentarily out of queue and did NOT apply the request.
 * The one retriable status: send it again after a short backoff. The
 * client already retried it (busyRetries, every operation but delete).
 */
final class BusyException extends ThunderstrikeException
{
}
