<?php

declare(strict_types=1);

namespace Thunderstrike;

/** The deadline elapsed. The server may still apply the request - thunderstrike has no cancel opcode. */
final class TimeoutException extends ThunderstrikeException
{
}
