namespace OfflineProtocol.Sdk;

internal static class OffThreadExecution
{
    /// <summary>
    /// Runs work off the UI synchronization context when one is present; otherwise runs inline (CLI / tests).
    /// </summary>
    internal static Task<T> RunAsync<T>(Func<T> work, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        if (SynchronizationContext.Current is null)
        {
            return Task.FromResult(work());
        }

        return Task.Run(
            () =>
            {
                cancellationToken.ThrowIfCancellationRequested();
                return work();
            },
            cancellationToken);
    }
}
