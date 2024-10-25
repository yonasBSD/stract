import type { RequestEvent } from '@sveltejs/kit';
import { clearExperiments } from '$lib/db';

export async function POST({}: RequestEvent): Promise<Response> {
	clearExperiments();
	return new Response('OK');
}
