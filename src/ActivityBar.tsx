export function ActivityBar({ label }: { label: string }) {
  return <div className="batch-progress" role="progressbar" aria-label={label}>
    <span className="progress-activity" aria-hidden="true" />
  </div>;
}
