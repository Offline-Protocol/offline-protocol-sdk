namespace OfflineProtocol.Sdk;

internal static class OffThreadExecution
{
    /// <summary>
    /// Runs work off the UI synchronization context when one is present; otherwise runs inline (CLI / tests).
    /// Every failure, a cancellation included, faults the returned task rather than throwing at the call.
    /// </summary>
    internal static async Task<T> RunAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        if (SynchronizationContext.Current is null)
        {
            return work();
        }

        return await Task.Run(
            () =>
            {
                cancellationToken.ThrowIfCancellationRequested();
                return work();
            },
            cancellationToken).ConfigureAwait(false);
    }
}
