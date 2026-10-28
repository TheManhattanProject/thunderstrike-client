<?php

declare(strict_types=1);

namespace Thunderstrike;

/** The key is absent. Only thrown by methods that promise a value; get() returns null instead. */
final class KeyNotFoundException extends ThunderstrikeException
{
}
