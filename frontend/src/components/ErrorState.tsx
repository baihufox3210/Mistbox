import { ApiError, contentUrl, errorText, type NodeInfo } from '../api';
import { Icon } from './Icon';

export function ErrorState({ error, node, retry }: { error: unknown; node?: NodeInfo; retry?: () => void }) {
  const status = error instanceof ApiError ? error.status : 0;
  return (
    <div className="state error" role="alert">
      <p>{status === 403 ? '沒有權限檢視此項目。' : status === 404 ? '找不到項目，可能已被移動或刪除。' : errorText(error)}</p>
      <div className="row">
        {retry && <button className="btn" onClick={retry}>重試</button>}
        {node && node.kind === 'file' && status !== 403 && status !== 404 && (
          <a className="btn" href={contentUrl(node.id)} download><Icon name="download" /> 下載</a>
        )}
      </div>
    </div>
  );
}

