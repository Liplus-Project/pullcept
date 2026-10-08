export const ADMISSION_WAIT_MS: number;
export const ADMISSION_INTERVAL_MS: number;
export const ADMISSION_PATH: string;
export function admitSidecar(options: {
  roomUrl: string; token: string;
  claim: { launch_id: string | null; account_id: string | null; room_id: string | null; instance_id: string };
}): Promise<boolean>;
