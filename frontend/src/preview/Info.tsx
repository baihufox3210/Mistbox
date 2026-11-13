import { contentUrl, type NodeInfo } from '../api';
import { FileIcon } from '../components/FileIcon';
import { Icon } from '../components/Icon';
import { formatDate, formatSize } from '../format';

export function Info({ node, reason }: { node: NodeInfo; reason?: string }) {
  return (
    <div className="info-view">
      <div className="info-icon"><FileIcon name={node.name} kind={node.kind} size={64} /></div>
      <h3>{node.name}</h3>
      <p className="muted">{reason ?? '此格式沒有內建預覽，但檔案已完整保存，可下載後使用。'}</p>
      <dl>
        <dt>大小</dt><dd>{formatSize(node.size)}（{node.size.toLocaleString()} bytes）</dd>
        <dt>類型</dt><dd>{node.mime ?? '未知'}</dd>
        <dt>修改時間</dt><dd>{formatDate(node.modified_at)}</dd>
        <dt>SHA-256</dt><dd className="mono wrap">{node.sha256}</dd>
      </dl>
      <a className="btn primary" href={contentUrl(node.id)} download>
        <Icon name="download" /> 下載
      </a>
    </div>
  );
}
