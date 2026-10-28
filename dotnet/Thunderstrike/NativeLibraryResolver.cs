#if NETCOREAPP3_0_OR_GREATER
using System;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;

namespace Thunderstrike;

/// <summary>
/// Teaches the runtime where to find the native library during
/// development.
/// </summary>
/// <remarks>
/// <para>
/// In a published app the library sits next to the assembly and the
/// default probing finds it. While developing against a checkout it
/// is in <c>target/release</c> instead, which nothing knows about —
/// so this resolver looks there, and honours
/// <c>THUNDERSTRIKE_CLIENT_LIB</c> for an explicit path, exactly like the
/// Python binding.
/// </para>
/// <para>
/// The resolver only supplements the default: returning
/// <see cref="IntPtr.Zero"/> hands the lookup back to the runtime, so
/// a properly deployed library still wins.
/// </para>
/// </remarks>
internal static class NativeLibraryResolver
{
    private static int _installed;

    /// <summary>Install once per process. Safe to call repeatedly.</summary>
    internal static void EnsureInstalled()
    {
        if (System.Threading.Interlocked.Exchange(ref _installed, 1) != 0) return;
        try
        {
            NativeLibrary.SetDllImportResolver(typeof(NativeLibraryResolver).Assembly, Resolve);
        }
        catch (InvalidOperationException)
        {
            // Another component already installed a resolver for this
            // assembly. Default probing still applies.
        }
    }

    private static IntPtr Resolve(string libraryName, Assembly assembly, DllImportSearchPath? path)
    {
        if (libraryName != NativeMethods.Lib) return IntPtr.Zero;

        foreach (var candidate in Candidates())
        {
            if (candidate is not null &&
                File.Exists(candidate) &&
                NativeLibrary.TryLoad(candidate, out var handle))
            {
                return handle;
            }
        }
        // Fall back to the runtime's own probing.
        return IntPtr.Zero;
    }

    private static System.Collections.Generic.IEnumerable<string?> Candidates()
    {
        yield return Environment.GetEnvironmentVariable("THUNDERSTRIKE_CLIENT_LIB");

        var fileName = FileName();
        var here = Path.GetDirectoryName(typeof(NativeLibraryResolver).Assembly.Location);
        if (here is null) yield break;

        yield return Path.Combine(here, fileName);

        // bindings/dotnet/Thunderstrike/bin/<cfg>/<tfm> -> the repo root is
        // six levels up; try a few depths rather than hard-coding one
        // build layout.
        var dir = here;
        for (var i = 0; i < 8 && dir is not null; i++)
        {
            yield return Path.Combine(dir, "target", "release", fileName);
            yield return Path.Combine(dir, "target", "debug", fileName);
            dir = Path.GetDirectoryName(dir);
        }
    }

    private static string FileName()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) return "thunderstrike_client.dll";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX)) return "libthunderstrike_client.dylib";
        return "libthunderstrike_client.so";
    }
}
#endif
